//! Recording armed tracks' inputs into the project (WP-2.13).
//!
//! Turning recording on gives the engine a [`RecordingPlan`]: a ring per
//! armed track and where its input's samples go. The engine's audio thread
//! copies them there, one part of a block at a time, with nothing allocated
//! or locked; a writer thread here drains the rings into WAVs in the
//! project's `Samples/Recorded/`, one per track. When the take ends, the
//! engine drops the plan, which closes the rings; the writer finishes the
//! files and reports them, and the shell makes the clips from them
//! ([`crate::Shell`]).
//!
//! An armed MIDI track records without a file: its ring holds the note
//! messages the engine stamped with song frames, the main thread drains it
//! as the take runs, and the clip's notes are made from the messages when
//! the take ends (WP-2.14).
//!
//! A ring that fills up (a writer that fell behind) is reported by the
//! engine's frames-dropped count, and a write that fails (a full disk) by
//! the writer's report: neither is ever silent, and a failed recording
//! leaves no partial file.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::{Duration, SystemTime};

use bl_core::time::{Frames, SampleRate, TempoMap, Ticks};
use bl_engine::{MidiRecordTrack, RecordTrack, RecordedMidi, RecordingPlan, TakeBounds};
use bl_media::{AudioWriter, BitDepth, Dither, EncodeSpec, FileType};
use bl_model::{
    Channel, Clip, ClipData, ClipId, Command, EditError, Editor, MidiData, Note, TrackId,
};
use rtrb::{Consumer, RingBuffer};

/// Frames of one track's samples the ring between the audio thread and the
/// writer holds, about 2.7 seconds of stereo at 48 kHz, so a writer that
/// stalls briefly loses nothing.
const RING_FRAMES: usize = 1 << 17;

/// Messages the ring between the audio thread and the main thread holds for
/// one MIDI track: far more than a person plays between two polls.
const MIDI_RING: usize = 1 << 14;

/// Samples the writer buffers before a write.
const CHUNK: usize = 8_192;

/// How long the writer sleeps when the rings are empty.
const POLL: Duration = Duration::from_millis(1);

/// The undo step a finished recording makes. Live's name for it isn't
/// observed yet.
pub const UNDO_LABEL: &str = "Recorded Clip";

/// One track to record: where its input comes from, and the name of the
/// file it goes to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Track {
    /// The track whose input is recorded.
    pub track: TrackId,
    /// The interface channels its Audio From names.
    pub channel: Channel,
    /// The file's name, without a directory ([`file_name`]).
    pub name: String,
}

/// The file name Live gives a recording: `<label> <take> [<stamp>].wav` with
/// the stamp of a backup's names, and characters that can't be in a file
/// name replaced by `-`. The label is the track's own (`5-Guitar`).
#[must_use]
pub fn file_name(label: &str, take: u32, stamp: &str) -> String {
    let clean: String = label
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                '-'
            } else {
                c
            }
        })
        .collect();
    let clean = clean.trim().trim_end_matches(['.', ' ']);
    let clean = if clean.is_empty() { "Audio" } else { clean };
    format!("{clean} {take:04} [{stamp}].wav")
}

/// The stamp recording file names carry, now, in UTC.
#[must_use]
pub fn stamp() -> String {
    bl_project::utc_stamp(SystemTime::now())
}

/// Starts a recording: one ring per track for the engine, the writer thread
/// draining the audio tracks' into WAVs under `Samples/Recorded/` in
/// `folder`, the project's root, 32-bit float at `rate`, and a ring for each
/// of the `midi` tracks. `folder` is only needed when `tracks` isn't empty.
///
/// # Errors
///
/// A message if `Samples/Recorded` can't be made or the writer thread can't
/// start.
pub fn start(
    folder: Option<&Path>,
    rate: SampleRate,
    tracks: &[Track],
    midi: &[TrackId],
) -> Result<(RecordingPlan, Recording), String> {
    let directory = folder
        .map(|folder| folder.join("Samples").join("Recorded"))
        .unwrap_or_default();
    if !tracks.is_empty() {
        std::fs::create_dir_all(&directory)
            .map_err(|e| format!("couldn't make {}: {e}", directory.display()))?;
    }
    let mut plan = Vec::with_capacity(tracks.len());
    let mut writing = Vec::with_capacity(tracks.len());
    let mut dropped = Vec::with_capacity(tracks.len());
    for track in tracks {
        let channels = track.channels();
        let (producer, consumer) = RingBuffer::new(RING_FRAMES * channels);
        let frames = Arc::new(AtomicU64::new(0));
        dropped.push(Arc::clone(&frames));
        writing.push(Writing {
            consumer,
            path: directory.join(&track.name),
            track: track.track,
            channels,
        });
        plan.push(RecordTrack {
            track: track.track,
            channel: track.channel,
            producer,
            dropped: frames,
        });
    }
    let (done, finished) = mpsc::channel();
    let thread = thread::Builder::new()
        .name("bl-shell-recording".into())
        .spawn(move || write(writing, rate, done))
        .map_err(|e| format!("couldn't start the recording writer: {e}"))?;
    drop(thread);
    let mut midi_plan = Vec::with_capacity(midi.len());
    let mut midi_reading = Vec::with_capacity(midi.len());
    for &track in midi {
        let (producer, consumer) = RingBuffer::new(MIDI_RING);
        let frames = Arc::new(AtomicU64::new(0));
        midi_plan.push(MidiRecordTrack {
            track,
            producer,
            dropped: Arc::clone(&frames),
        });
        midi_reading.push(MidiReading {
            track,
            consumer,
            messages: Vec::new(),
            dropped: frames,
        });
    }
    Ok((
        RecordingPlan {
            tracks: plan,
            midi: midi_plan,
            delay: 0,
        },
        Recording {
            armed: tracks
                .iter()
                .map(|track| track.track)
                .chain(midi.iter().copied())
                .collect(),
            finished,
            files: None,
            dropped,
            midi: midi_reading,
            take: None,
            delay: 0,
        },
    ))
}

impl Track {
    /// How many samples one frame has: one for a mono channel, two for a
    /// pair.
    #[must_use]
    pub fn channels(&self) -> usize {
        match self.channel {
            Channel::Mono(_) => 1,
            Channel::Pair(_) => 2,
        }
    }
}

/// One track's side of the recording, on the main thread: the writer's
/// report, the frames dropped, and the take the engine published.
pub struct Recording {
    /// The armed tracks it records: audio, then MIDI.
    armed: Vec<TrackId>,
    finished: Receiver<Result<Vec<RecordedFile>, String>>,
    /// The writer's report, kept while the engine's take state catches up.
    files: Option<Result<Vec<RecordedFile>, String>>,
    dropped: Vec<Arc<AtomicU64>>,
    /// The MIDI tracks' rings, drained at every poll.
    midi: Vec<MidiReading>,
    take: Option<TakeBounds>,
    /// Frames the audio is moved earlier (WP-2.13): the plan's `delay`,
    /// which the shell sets once it knows the device.
    pub delay: i64,
}

/// One MIDI track's ring, with what has been drained from it.
struct MidiReading {
    track: TrackId,
    consumer: Consumer<RecordedMidi>,
    messages: Vec<RecordedMidi>,
    dropped: Arc<AtomicU64>,
}

/// The note messages one MIDI track received while the take ran.
#[derive(Debug)]
pub struct MidiTake {
    /// The track.
    pub track: TrackId,
    /// Its notes' ons and offs, in order.
    pub messages: Vec<RecordedMidi>,
}

/// What a finished recording came to.
#[derive(Debug)]
pub struct Outcome {
    /// The take the engine published, if it saw one: the song frames the
    /// clips are placed from. `None` when nothing was played.
    pub take: Option<TakeBounds>,
    /// Frames the audio was moved earlier: its clips start and end that much
    /// earlier than the take, and its files leave out the take's first
    /// frames up to the delay.
    pub delay: i64,
    /// The finished files, or why the writer gave up.
    pub files: Result<Vec<RecordedFile>, String>,
    /// What each MIDI track that received a note recorded.
    pub midi: Vec<MidiTake>,
    /// Frames (and MIDI messages) a ring was too full to take: the disk or
    /// the main thread fell behind, and the recording ended before them.
    pub dropped: u64,
}

/// One file the writer finished.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedFile {
    /// The track it holds.
    pub track: TrackId,
    /// The file.
    pub path: PathBuf,
    /// The frames written.
    pub frames: u64,
    /// Its channels.
    pub channels: usize,
}

impl Recording {
    /// The armed tracks it records: audio, then MIDI.
    #[must_use]
    pub fn armed(&self) -> &[TrackId] {
        &self.armed
    }

    /// Frames and MIDI messages dropped so far.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped
            .iter()
            .chain(self.midi.iter().map(|reading| &reading.dropped))
            .map(|d| d.load(Relaxed))
            .sum()
    }

    /// Notes the take the engine published, from its state.
    pub fn note_take(&mut self, take: TakeBounds) {
        self.take = Some(take);
    }

    /// What the recording came to, once it has: the writer has reported and
    /// the engine's take state has caught up. `None` while either is still
    /// on its way, so the caller polls again. `running` is whether an engine
    /// still runs: with none, the take can't be published any more, and
    /// what the writer finished is reported as it is.
    #[must_use]
    pub fn poll(&mut self, running: bool) -> Option<Outcome> {
        let mut ended = true;
        for reading in &mut self.midi {
            // A ring abandoned before the drain has nothing more to come.
            ended &= reading.consumer.is_abandoned();
            while let Ok(message) = reading.consumer.pop() {
                reading.messages.push(message);
            }
        }
        if self.files.is_none() {
            match self.finished.try_recv() {
                Ok(files) => self.files = Some(files),
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => {
                    self.files = Some(Err("the recording writer stopped".to_owned()));
                }
            }
        }
        if !ended {
            return None;
        }
        let recorded = matches!(&self.files, Some(Ok(files)) if !files.is_empty())
            || self.midi.iter().any(|reading| !reading.messages.is_empty());
        if recorded && self.take.is_none() && running {
            // The take state is published at the end of the block the plan
            // went in, so it can arrive a frame after the writer's report.
            return None;
        }
        let dropped = self.dropped();
        let midi = self
            .midi
            .drain(..)
            .filter(|reading| !reading.messages.is_empty())
            .map(|reading| MidiTake {
                track: reading.track,
                messages: reading.messages,
            })
            .collect();
        Some(Outcome {
            take: self.take,
            delay: self.delay,
            files: self.files.take().expect("set above"),
            midi,
            dropped,
        })
    }
}

impl std::fmt::Debug for Recording {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Recording")
            .field("dropped", &self.dropped.len())
            .field("midi", &self.midi.len())
            .field("take", &self.take)
            .finish_non_exhaustive()
    }
}

/// One track's writer: its ring's reading end, the file to write, and what
/// it writes.
struct Writing {
    consumer: Consumer<f32>,
    path: PathBuf,
    track: TrackId,
    channels: usize,
}

/// The writer thread: drains each track's ring into its file until the
/// engine drops the plan (the take is over), then finishes the files and
/// reports them. Real-time safety binds the audio thread only; here a file
/// write may block as long as it likes.
fn write(
    mut tracks: Vec<Writing>,
    rate: SampleRate,
    done: Sender<Result<Vec<RecordedFile>, String>>,
) {
    let mut writers: Vec<Option<AudioWriter>> = (0..tracks.len()).map(|_| None).collect();
    let mut buffers: Vec<Vec<f32>> = tracks.iter().map(|_| Vec::with_capacity(CHUNK)).collect();
    let mut error: Option<String> = None;
    loop {
        let mut idle = true;
        for (index, track) in tracks.iter_mut().enumerate() {
            let channels = track.channels;
            let whole = CHUNK - CHUNK % channels;
            while let Ok(sample) = track.consumer.pop() {
                idle = false;
                if error.is_some() {
                    // The recording failed: keep draining, so a ring never
                    // fills and has the audio thread drop frames too.
                    continue;
                }
                if writers[index].is_none() {
                    match writer(&track.path, track.channels, rate) {
                        Ok(opened) => writers[index] = Some(opened),
                        Err(e) => {
                            error = Some(e);
                            continue;
                        }
                    }
                }
                buffers[index].push(sample);
                if buffers[index].len() >= whole {
                    let write = writers[index]
                        .as_mut()
                        .expect("opened above")
                        .write(&buffers[index]);
                    if let Err(e) = write {
                        error = Some(format!("{}: {e}", track.path.display()));
                    }
                    buffers[index].clear();
                }
            }
        }
        if tracks.iter().all(|track| track.consumer.is_abandoned()) && idle {
            break;
        }
        thread::sleep(POLL);
    }
    let mut files = Vec::with_capacity(tracks.len());
    for (index, track) in tracks.iter_mut().enumerate() {
        if error.is_some() {
            break;
        }
        let Some(mut writer) = writers[index].take() else {
            // The take never reached this track: no file.
            continue;
        };
        if !buffers[index].is_empty()
            && let Err(e) = writer.write(&buffers[index])
        {
            error = Some(format!("{}: {e}", track.path.display()));
            break;
        }
        match writer.finish() {
            Ok(frames) => files.push(RecordedFile {
                track: track.track,
                path: track.path.clone(),
                frames,
                channels: track.channels,
            }),
            Err(e) => error = Some(format!("{}: {e}", track.path.display())),
        }
    }
    match error {
        Some(error) => {
            // A failed recording leaves no partial file, as a failed export
            // leaves none.
            for track in &tracks {
                let _ = std::fs::remove_file(&track.path);
            }
            let _ = done.send(Err(error));
        }
        None => {
            let _ = done.send(Ok(files));
        }
    }
}

/// A writer for one track's file: WAV, 32-bit float, so the recorded
/// samples are the interface's to the bit.
fn writer(path: &Path, channels: usize, rate: SampleRate) -> Result<AudioWriter, String> {
    AudioWriter::create(
        path,
        EncodeSpec {
            file_type: FileType::Wav,
            bit_depth: BitDepth::Float32,
            dither: Dither::None,
            sample_rate: rate.hz(),
            channels: channels as u16,
        },
    )
    .map_err(|e| format!("{}: {e}", path.display()))
}

/// The notes a MIDI track's messages played, as absolute positions: each
/// note-on starts a note on its key, and the next note-off on that key (or a
/// note-on on it again) ends it. A note still held when the take ends at
/// `end` ends there; a note-off with no note held is ignored. Every note
/// lasts at least a tick.
fn notes(messages: &[RecordedMidi], end: Frames, tempo: &TempoMap, rate: SampleRate) -> Vec<Note> {
    let ticks = |frames| tempo.frames_to_ticks(frames, rate);
    let finish = |key: u8, (from, velocity): (Frames, u8), to: Frames, release: u8| {
        let start = ticks(from);
        let length = (ticks(to) - start).max(Ticks(1));
        let mut note = Note::new(key, start, length, velocity.max(1));
        // A note-off without a velocity of its own is the usual 64.
        note.release_velocity = if release == 0 { 64 } else { release };
        note
    };
    let mut held: [Option<(Frames, u8)>; 128] = [None; 128];
    let mut notes = Vec::new();
    for RecordedMidi { frame, message } in messages {
        let key = message.data1();
        let on = message.is_note_on();
        if let Some(note) = held[usize::from(key)].take() {
            notes.push(finish(
                key,
                note,
                *frame,
                if on { 0 } else { message.data2() },
            ));
        }
        if on {
            held[usize::from(key)] = Some((*frame, message.data2()));
        }
    }
    for (key, note) in (0u8..).zip(held) {
        notes.extend(note.map(|note| finish(key, note, end, 0)));
    }
    notes
}

/// Puts what MIDI tracks recorded into the Arrangement, inside the caller's
/// transaction: a new clip over the take on each track that received notes,
/// replacing what was there; or, with `overdub` on, each note merged into
/// the MIDI clip it starts in, and the notes that start in no clip making
/// new clips over the gaps between the track's clips, so nothing is replaced
/// (WP-2.14). `placed` holds the clips already made, which clearing leaves
/// alone.
///
/// # Errors
///
/// [`EditError`] if the document refuses a step.
pub fn place_midi(
    editor: &mut Editor,
    takes: &[MidiTake],
    take: TakeBounds,
    rate: SampleRate,
    overdub: bool,
    placed: &mut std::collections::BTreeSet<ClipId>,
) -> Result<(), EditError> {
    let tempo = editor.document().settings().tempo.clone();
    let (start, end) = (
        tempo.frames_to_ticks(take.start, rate),
        tempo.frames_to_ticks(take.end, rate),
    );
    for recorded in takes {
        let mut played = notes(&recorded.messages, take.end, &tempo, rate);
        if played.is_empty() {
            continue;
        }
        // The spans of the track's clips the take crosses, in order of
        // start: the track holds them in order of ID.
        let mut clips: Vec<(ClipId, Ticks, Ticks, Ticks)> = editor
            .document()
            .track(recorded.track)
            .map(|lane| {
                lane.clips()
                    .filter(|c| overdub && c.start < end && c.end() > start)
                    .filter(|c| matches!(c.data, ClipData::Midi(_)))
                    .map(|c| (c.id, c.start, c.end(), c.content_start()))
                    .collect()
            })
            .unwrap_or_default();
        clips.sort_unstable_by_key(|&(_, start, _, _)| start);
        // Each note goes to the clip it starts in.
        for (id, from, to, origin) in &clips {
            let (inside, rest): (Vec<_>, Vec<_>) = played
                .into_iter()
                .partition(|note| note.start >= *from && note.start < *to);
            played = rest;
            if inside.is_empty() {
                continue;
            }
            let Some(ClipData::Midi(mut data)) =
                editor.document().clip(*id).map(|clip| clip.data.clone())
            else {
                continue;
            };
            for note in inside {
                data.notes.push_back(Note {
                    start: note.start - *origin,
                    ..note
                });
            }
            editor.apply(Command::SetClipData {
                clip: *id,
                data: ClipData::Midi(data),
            })?;
        }
        if played.is_empty() {
            continue;
        }
        // What is left makes new clips: over the take, or over the gaps the
        // track's clips leave in it.
        let mut gaps = Vec::new();
        let mut from = start;
        for (_, clip_start, clip_end, _) in &clips {
            if *clip_start > from {
                gaps.push((from, *clip_start));
            }
            from = from.max(*clip_end);
        }
        if from < end {
            gaps.push((from, end));
        }
        for (from, to) in gaps {
            let inside: Vec<Note> = played
                .iter()
                .filter(|note| note.start >= from && note.start < to)
                .map(|note| Note {
                    start: note.start - from,
                    ..*note
                })
                .collect();
            if inside.is_empty() {
                continue;
            }
            let id = editor.new_id();
            let clip = Clip::new(id, from, to - from, ClipData::Midi(MidiData::new(inside)));
            crate::arrangement::placing::clear(editor, recorded.track, from, to, placed)?;
            placed.insert(id);
            editor.apply(Command::CreateClip {
                track: recorded.track,
                clip,
            })?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use bl_core::midi::MidiMessage;

    use super::*;

    #[test]
    fn file_names_are_lives_and_hold_no_path_characters() {
        assert_eq!(
            file_name("5-Guitar", 1, "2026-10-15 180000"),
            "5-Guitar 0001 [2026-10-15 180000].wav"
        );
        assert_eq!(
            file_name("2-Bass / Pad", 12, "2026-10-15 180000"),
            "2-Bass - Pad 0012 [2026-10-15 180000].wav"
        );
        assert_eq!(
            file_name("   ", 1, "2026-10-15 180000"),
            "Audio 0001 [2026-10-15 180000].wav"
        );
    }

    // Spec: app-shell R13; WP-2.13. A recording that ends with frames the
    // engine dropped reports them, and a take that never started makes no
    // file.
    #[test]
    fn a_recordings_report_carries_its_dropped_frames() {
        let dir = std::env::temp_dir().join(format!(
            "bl-shell-recording-unit-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let track = TrackId(bl_core::id::Id::new(1).unwrap());
        let (plan, mut recording) = start(
            Some(&dir),
            SampleRate::HZ_48000,
            &[Track {
                track,
                channel: Channel::Mono(3),
                name: file_name("1-Audio", 1, "2026-10-15 180000"),
            }],
            &[],
        )
        .unwrap();
        let dropped = Arc::clone(&plan.tracks[0].dropped);
        // The take ends (the plan goes), and the engine had dropped frames.
        drop(plan);
        dropped.fetch_add(9, Relaxed);
        let outcome = loop {
            if let Some(outcome) = recording.poll(false) {
                break outcome;
            }
            std::thread::sleep(Duration::from_millis(1));
        };
        assert_eq!(outcome.dropped, 9);
        assert_eq!(outcome.files.unwrap(), Vec::new(), "no samples, no file");
        let _ = std::fs::remove_dir_all(&dir);
    }

    const RATE: SampleRate = SampleRate::HZ_48000;

    fn midi(frame: i64, message: MidiMessage) -> RecordedMidi {
        RecordedMidi {
            frame: Frames(frame),
            message,
        }
    }

    /// An editor holding a MIDI track.
    fn editor_with_track() -> (Editor, TrackId) {
        let mut editor = Editor::new();
        let id = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id,
                kind: bl_model::TrackKind::Midi,
                name: String::new(),
                parent: None,
                index: 0,
            })
            .unwrap();
        (editor, id)
    }

    /// A MIDI clip of `bars` bars at `bar`, holding one note at its start.
    fn add_clip(editor: &mut Editor, track: TrackId, bar: i64, bars: i64, key: u8) -> ClipId {
        let id = editor.new_id();
        let start = Ticks::WHOLE * bar;
        let note = Note::new(key, Ticks::ZERO, Ticks::QUARTER, 100);
        editor
            .apply(Command::CreateClip {
                track,
                clip: Clip::new(
                    id,
                    start,
                    Ticks::WHOLE * bars,
                    ClipData::Midi(MidiData::new([note])),
                ),
            })
            .unwrap();
        id
    }

    /// The track's MIDI clips as (start in bars, length in bars, the notes'
    /// keys in order of start).
    fn clips(editor: &Editor, track: TrackId) -> Vec<(i64, i64, Vec<u8>)> {
        let mut found: Vec<_> = editor
            .document()
            .track(track)
            .unwrap()
            .clips()
            .map(|clip| {
                let ClipData::Midi(data) = &clip.data else {
                    panic!("a MIDI clip")
                };
                let mut notes: Vec<_> = data.notes.iter().collect();
                notes.sort_by_key(|note| note.start);
                (
                    clip.start.get() / Ticks::WHOLE.get(),
                    clip.length.get() / Ticks::WHOLE.get(),
                    notes.iter().map(|note| note.key).collect(),
                )
            })
            .collect();
        found.sort();
        found
    }

    /// A take of the bars `from` to `to` at 120 BPM: a bar is 96,000 frames.
    fn take(from: i64, to: i64) -> TakeBounds {
        TakeBounds {
            start: Frames(from * 96_000),
            end: Frames(to * 96_000),
        }
    }

    fn played(track: TrackId, frames: &[(i64, MidiMessage)]) -> Vec<MidiTake> {
        vec![MidiTake {
            track,
            messages: frames.iter().map(|&(f, m)| midi(f, m)).collect(),
        }]
    }

    // Spec: WP-2.14. A note-on starts a note and the next note-off on its
    // key ends it; a note still held when the take ends ends there, and a
    // note-off with nothing held is ignored.
    #[test]
    fn notes_pair_ons_with_offs_and_close_held_ones_at_the_take_end() {
        let tempo = TempoMap::default();
        let messages = [
            midi(0, MidiMessage::note_off(0, 70)),
            midi(100, MidiMessage::note_on(0, 60, 90)),
            midi(300, MidiMessage::note_off(0, 60)),
            midi(400, MidiMessage::note_on(0, 62, 80)),
            midi(500, MidiMessage::note_on(0, 62, 70)),
            midi(600, MidiMessage::note_on(0, 64, 60)),
        ];
        let mut found = notes(&messages, Frames(1_000), &tempo, RATE);
        found.sort_by_key(|note| note.start);
        let shape: Vec<_> = found
            .iter()
            .map(|n| {
                (
                    n.key,
                    n.velocity,
                    tempo.ticks_to_frames(n.start, RATE).get(),
                    tempo.ticks_to_frames(n.start + n.length, RATE).get(),
                )
            })
            .collect();
        assert_eq!(
            shape,
            vec![
                (60, 90, 100, 300),
                (62, 80, 400, 500),
                (62, 70, 500, 1_000),
                (64, 60, 600, 1_000),
            ]
        );
    }

    // Spec: WP-2.14. Without overdub a take makes one clip over it on each
    // track that received notes, replacing what was there, with every note
    // on the sample it was played on.
    #[test]
    fn a_take_makes_a_clip_over_it_and_replaces_what_was_there() {
        let (mut editor, track) = editor_with_track();
        add_clip(&mut editor, track, 0, 4, 40);
        let (on, off) = (
            MidiMessage::note_on(0, 60, 100),
            MidiMessage::note_off(0, 60),
        );
        let takes = played(track, &[(96_000 + 12_345, on), (96_000 + 20_000, off)]);
        let mut placed = Default::default();
        place_midi(&mut editor, &takes, take(1, 2), RATE, false, &mut placed).unwrap();

        // The old clip keeps bar 0 and bars 2 to 3; the take is bar 1.
        assert_eq!(
            clips(&editor, track),
            vec![(0, 1, vec![40]), (1, 1, vec![60]), (2, 2, vec![40])]
        );
        let tempo = editor.document().settings().tempo.clone();
        let clip = editor
            .document()
            .track(track)
            .unwrap()
            .clips()
            .find(|clip| clip.start == Ticks::WHOLE)
            .unwrap();
        assert_eq!(tempo.ticks_to_frames(clip.start, RATE).get(), 96_000);
        let ClipData::Midi(data) = &clip.data else {
            panic!("a MIDI clip")
        };
        let note = data.notes.iter().next().unwrap();
        assert_eq!(
            tempo.ticks_to_frames(clip.start + note.start, RATE).get(),
            96_000 + 12_345,
            "the note starts on the sample it was played on"
        );
        assert_eq!(
            tempo
                .ticks_to_frames(clip.start + note.start + note.length, RATE)
                .get(),
            96_000 + 20_000
        );
    }

    // Spec: WP-2.14. With overdub on, a note that starts in a clip joins
    // it, and the clip keeps its own notes.
    #[test]
    fn overdub_adds_the_notes_to_the_clip_they_start_in() {
        let (mut editor, track) = editor_with_track();
        let clip = add_clip(&mut editor, track, 0, 2, 40);
        let on = MidiMessage::note_on(0, 60, 100);
        let off = MidiMessage::note_off(0, 60);
        let takes = played(track, &[(48_000, on), (60_000, off)]);
        let mut placed = Default::default();
        place_midi(&mut editor, &takes, take(0, 2), RATE, true, &mut placed).unwrap();

        assert_eq!(clips(&editor, track), vec![(0, 2, vec![40, 60])]);
        assert_eq!(
            editor
                .document()
                .track(track)
                .unwrap()
                .clips()
                .next()
                .unwrap()
                .id,
            clip,
            "the same clip"
        );
    }

    // Spec: WP-2.14. With overdub on, a note that starts where no clip is
    // makes a clip over the gap, and the clips on either side stay as they
    // were.
    #[test]
    fn overdub_fills_the_gaps_between_clips_without_replacing_any() {
        let (mut editor, track) = editor_with_track();
        add_clip(&mut editor, track, 1, 1, 40);
        let in_gap = MidiMessage::note_on(0, 60, 100);
        let in_clip = MidiMessage::note_on(0, 62, 100);
        let after = MidiMessage::note_on(0, 64, 100);
        let takes = played(
            track,
            &[
                (10_000, in_gap),
                (96_000 + 10_000, in_clip),
                (2 * 96_000 + 10_000, after),
            ],
        );
        let mut placed = Default::default();
        place_midi(&mut editor, &takes, take(0, 3), RATE, true, &mut placed).unwrap();

        assert_eq!(
            clips(&editor, track),
            vec![(0, 1, vec![60]), (1, 1, vec![40, 62]), (2, 1, vec![64])]
        );
    }

    // Spec: WP-2.14. The gaps follow the clips' positions, not the order
    // they were made in: a clip made later but placed earlier is kept.
    #[test]
    fn overdub_finds_the_gaps_whatever_order_the_clips_were_made_in() {
        let (mut editor, track) = editor_with_track();
        add_clip(&mut editor, track, 2, 1, 40);
        add_clip(&mut editor, track, 0, 1, 42);
        let takes = played(
            track,
            &[(96_000 + 48_000, MidiMessage::note_on(0, 60, 100))],
        );
        let mut placed = Default::default();
        place_midi(&mut editor, &takes, take(0, 4), RATE, true, &mut placed).unwrap();

        assert_eq!(
            clips(&editor, track),
            vec![(0, 1, vec![42]), (1, 1, vec![60]), (2, 1, vec![40])]
        );
    }

    // Spec: WP-2.14. A track that received no notes gets no clip.
    #[test]
    fn a_take_without_notes_makes_no_clip() {
        let (mut editor, track) = editor_with_track();
        let mut placed = Default::default();
        place_midi(&mut editor, &[], take(0, 1), RATE, false, &mut placed).unwrap();
        assert!(clips(&editor, track).is_empty());
    }
}
