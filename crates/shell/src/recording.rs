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
//! An armed MIDI track records without a file: its ring holds the note and
//! controller messages the engine stamped with song frames, the main thread
//! drains it as the take runs, and the clip's notes and controller envelopes
//! are made from the messages when the take ends (WP-2.14, WP-2.29b).
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

use bl_core::midi::MidiMessage;
use bl_core::time::{Frames, SampleRate, TempoMap, Ticks};
use bl_engine::controller_take::{self, REST};
use bl_engine::mpe::MpeRole;
use bl_engine::{
    LaunchLog, MidiRecordTrack, RecordTrack, RecordedMidi, RecordingPlan, SessionEvent,
    SessionTarget, TakeBounds,
};
use bl_media::{AudioWriter, BitDepth, Dither, EncodeSpec, FileType};
use bl_model::{
    AutomationTarget, Channel, Clip, ClipData, ClipId, Command, EditError, Editor, EnvelopeKind,
    MidiData, NewTakeLane, Note, TakeLaneId, TrackId,
};
use rtrb::{Consumer, RingBuffer};
use std::collections::BTreeMap;

/// Frames of one track's samples the ring between the audio thread and the
/// writer holds, about 2.7 seconds of stereo at 48 kHz, so a writer that
/// stalls briefly loses nothing.
const RING_FRAMES: usize = 1 << 17;

/// Messages the ring between the audio thread and the main thread holds for
/// one MIDI track: far more than a person plays between two polls.
const MIDI_RING: usize = 1 << 14;

/// Session clip starts and ends the ring between the audio thread and the
/// main thread holds: a person launches a few a second.
const LAUNCH_RING: usize = 1 << 10;

/// Samples the writer buffers before a write.
const CHUNK: usize = 8_192;

/// How long the writer sleeps when the rings are empty.
const POLL: Duration = Duration::from_millis(1);

/// The undo step a finished recording makes. Live's name for it isn't
/// observed yet.
pub const UNDO_LABEL: &str = "Recorded Clip";

/// The undo step each pass of a loop recording makes, so Undo run again and
/// again unrolls the passes from the last (the recording spec, R12), and the
/// one a recording that copied Session clips makes (the Session to
/// Arrangement spec, R6).
pub const LOOP_UNDO_LABEL: &str = "Record";

/// The take lane a take starting at `start` goes on (the comping spec, R11):
/// the first of the track's take lanes, nearest the main lane first, with no
/// clip ending after `start`, or else a new lane after the last, named as
/// Insert Take Lane names it.
///
/// # Errors
///
/// [`EditError`] if the document refuses a step.
pub(crate) fn free_lane(
    editor: &mut Editor,
    track: TrackId,
    start: Ticks,
) -> Result<TakeLaneId, EditError> {
    let held = editor.document().track(track);
    if let Some(lane) = held.and_then(|held| {
        held.take_lanes()
            .find(|lane| lane.clips.iter().all(|clip| clip.end() <= start))
    }) {
        return Ok(lane.id);
    }
    let name = held
        .map(bl_model::Track::next_take_name)
        .unwrap_or_default();
    let id: TakeLaneId = editor.new_id();
    editor.apply(Command::AddTakeLane {
        track,
        lane: NewTakeLane {
            id: Some(id),
            name,
            clips: Vec::new(),
        },
        index: None,
    })?;
    Ok(id)
}

/// Puts a recorded clip on the take lane [`free_lane`] finds for it.
///
/// # Errors
///
/// [`EditError`] if the document refuses a step.
pub(crate) fn put_on_lane(
    editor: &mut Editor,
    track: TrackId,
    clip: Clip,
) -> Result<(), EditError> {
    let lane = free_lane(editor, track, clip.start)?;
    editor.apply(Command::CreateTakeClip { lane, clip })?;
    Ok(())
}

/// One pass of a loop recording: the song frame it starts at, where its
/// audio starts in the recorded file, and how many frames it holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pass {
    /// The song frame the pass starts at.
    pub song_start: Frames,
    /// The frame of the file the pass starts at.
    pub file_from: u64,
    /// How many frames the pass holds.
    pub frames: u64,
}

/// Splits a take that went round the loop into its parts (the recording
/// spec, R12): the lead-in before the loop's start, if recording began before
/// it, and then each pass round the loop, the first from where recording
/// began if that was inside the loop, the last as far as it got. `file` is
/// how many frames the file holds.
#[must_use]
pub fn passes(take: TakeBounds, file: u64) -> (Option<Pass>, Vec<Pass>) {
    let Some((start, end)) = take.wrap else {
        let whole = Pass {
            song_start: take.start,
            file_from: 0,
            frames: file,
        };
        return (None, vec![whole]);
    };
    let mut from = 0;
    let mut lead_in = None;
    let mut at = take.start;
    if take.start < start {
        let frames = ((start - take.start).get() as u64).min(file);
        lead_in = Some(Pass {
            song_start: take.start,
            file_from: 0,
            frames,
        });
        from = frames;
        at = start;
    }
    let mut passes = Vec::new();
    while from < file {
        let frames = ((end - at).get() as u64).min(file - from);
        passes.push(Pass {
            song_start: at,
            file_from: from,
            frames,
        });
        from += frames;
        at = start;
    }
    (lead_in, passes)
}

/// A MIDI take cut to the pass it holds: the messages that fall in the file
/// frames `pass` covers, with their frames put back to the song frames the
/// pass plays at. A message's frame in a take that went round the loop is
/// the take's running frame count, which counts as the take's file does.
#[must_use]
pub fn midi_in_pass(takes: &[MidiTake], take: TakeBounds, pass: &Pass) -> Vec<MidiTake> {
    takes
        .iter()
        .map(|recorded| MidiTake {
            track: recorded.track,
            messages: recorded
                .messages
                .iter()
                .filter_map(|m| {
                    let at = u64::try_from((m.frame - take.start).get()).ok()?;
                    let into = at.checked_sub(pass.file_from)?;
                    (into < pass.frames).then_some(RecordedMidi {
                        frame: pass.song_start + Frames(into as i64),
                        message: m.message,
                        mpe: m.mpe,
                    })
                })
                .collect(),
        })
        .collect()
}

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
/// `session` names the slots a Session recording goes into (WP-3.11); empty
/// for the Arrangement's.
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
    session: Vec<SessionTarget>,
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
    let (launch_producer, launches) = RingBuffer::new(LAUNCH_RING);
    let launches_dropped = Arc::new(AtomicU64::new(0));
    dropped.push(Arc::clone(&launches_dropped));
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
            session,
            launches: Some(LaunchLog {
                producer: launch_producer,
                dropped: launches_dropped,
            }),
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
            launches,
            events: Vec::new(),
            take: None,
            engines_closed: 0,
            stale: None,
            fresh: false,
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
    /// The ring the Session clips that start and end go through, drained at
    /// every poll (WP-3.13), and what has been drained.
    launches: Consumer<SessionEvent>,
    events: Vec<SessionEvent>,
    take: Option<TakeBounds>,
    /// The audio host's [`engines_closed`](crate::audio::AudioHost::engines_closed)
    /// when the recording began: another count means the engine that took
    /// it was closed, and no take will ever be published.
    engines_closed: u64,
    /// The take the engine still published when the recording began, which
    /// is the last recording's until the engine drains this one's plan.
    stale: Option<TakeBounds>,
    /// Whether the engine has shown a take other than the stale one: it
    /// cleared the old take, or is running a new one.
    fresh: bool,
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

/// The note and controller messages one MIDI track received while the take
/// ran.
#[derive(Debug)]
pub struct MidiTake {
    /// The track.
    pub track: TrackId,
    /// Its notes' ons and offs and its controllers, in order.
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
    /// What each MIDI track that received a note or a controller recorded.
    pub midi: Vec<MidiTake>,
    /// Frames (and MIDI messages) a ring was too full to take: the disk or
    /// the main thread fell behind, and the recording ended before them.
    pub dropped: u64,
    /// The Session clips that started and ended while the take ran, in
    /// order (WP-3.13).
    pub launches: Vec<SessionEvent>,
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

    /// Frames, MIDI messages and Session launches dropped so far.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped
            .iter()
            .chain(self.midi.iter().map(|reading| &reading.dropped))
            .map(|d| d.load(Relaxed))
            .sum()
    }

    /// The note messages a MIDI track has received so far, as the polls have
    /// drained them (WP-3.11).
    #[must_use]
    pub fn midi_so_far(&self, track: TrackId) -> &[RecordedMidi] {
        self.midi
            .iter()
            .find(|reading| reading.track == track)
            .map_or(&[], |reading| &reading.messages)
    }

    /// Notes the engine's state when the recording begins, for
    /// [`Recording::following`] and [`Recording::engine_closed`].
    pub fn began_on(&mut self, audio: &mut crate::audio::AudioHost) {
        self.engines_closed = audio.engines_closed();
        self.stale = audio.transport_state().take;
    }

    /// Whether the engine this recording began on was closed since, so no
    /// take will be published for it.
    #[must_use]
    pub fn engine_closed(&self, audio: &crate::audio::AudioHost) -> bool {
        self.engines_closed != audio.engines_closed()
    }

    /// Notes the take the engine published, from its state: `take` is the
    /// take that last ended and `running` whether one runs now. The take
    /// that ended before this recording began is not this recording's, and
    /// is still published until the engine has the new plan.
    pub fn follow(&mut self, take: Option<TakeBounds>, running: bool) {
        self.fresh |= take.is_none() || running;
        if let Some(take) = take
            && (self.fresh || self.stale != Some(take))
        {
            self.take = Some(take);
        }
    }

    /// What the recording came to, once it has: the writer has reported and
    /// the engine's take state has caught up. `None` while either is still
    /// on its way, so the caller polls again. `running` is whether an engine
    /// still runs: with none, the take can't be published any more, and
    /// what the writer finished is reported as it is.
    #[must_use]
    pub fn poll(&mut self, running: bool) -> Option<Outcome> {
        let mut ended = self.launches.is_abandoned();
        while let Ok(event) = self.launches.pop() {
            self.events.push(event);
        }
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
            || self.midi.iter().any(|reading| !reading.messages.is_empty())
            || !self.events.is_empty();
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
            launches: std::mem::take(&mut self.events),
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
/// lasts at least a tick, and carries the polyphonic pressure its key was
/// sent while it was held (the MIDI controller data spec, R11). A note from
/// an MPE port starts and ends by the ID the input gave it, and carries the
/// pitch, slide and pressure it was sent, stepped (the MPE spec, R12).
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
    // The notes an MPE port holds, by ID, and the ID of every note made.
    let mut mpe: Vec<(u32, u8, (Frames, u8))> = Vec::new();
    let mut ids: Vec<Option<u32>> = Vec::new();
    let mut heard: Vec<controller_take::Heard> = Vec::new();
    let mut notes = Vec::new();
    for RecordedMidi {
        frame,
        message,
        mpe: role,
    } in messages
    {
        let on = message.is_note_on();
        let release = if on { 0 } else { message.data2() };
        match *role {
            MpeRole::Expression {
                id,
                expression,
                value,
                ..
            } => heard.push(controller_take::Heard {
                id,
                expression,
                value: f64::from(value),
                at: ticks(*frame),
            }),
            MpeRole::Note { id } => {
                if let Some(at) = mpe.iter().position(|(held, ..)| *held == id) {
                    let (_, key, note) = mpe.remove(at);
                    notes.push(finish(key, note, *frame, release));
                    ids.push(Some(id));
                } else if on {
                    mpe.push((id, message.data1(), (*frame, message.data2())));
                }
            }
            MpeRole::Plain => {
                if !(on || message.is_note_off()) {
                    continue;
                }
                let key = message.data1();
                if let Some(note) = held[usize::from(key)].take() {
                    notes.push(finish(key, note, *frame, release));
                    ids.push(None);
                }
                if on {
                    held[usize::from(key)] = Some((*frame, message.data2()));
                }
            }
        }
    }
    for (key, note) in (0u8..).zip(held) {
        if let Some(note) = note {
            notes.push(finish(key, note, end, 0));
            ids.push(None);
        }
    }
    for (id, key, note) in mpe {
        notes.push(finish(key, note, end, 0));
        ids.push(Some(id));
    }
    controller_take::give_pressure(&mut notes, &moves(messages, tempo, rate));
    controller_take::give_expressions(&mut notes, &ids, &heard);
    notes
}

/// The controller messages a MIDI track received, as absolute positions.
fn moves(
    messages: &[RecordedMidi],
    tempo: &TempoMap,
    rate: SampleRate,
) -> Vec<(Ticks, MidiMessage)> {
    messages
        .iter()
        .filter(|m| m.mpe == MpeRole::Plain && controller_take::is_controller(&m.message))
        .map(|m| (tempo.frames_to_ticks(m.frame, rate), m.message))
        .collect()
}

/// The control change, pitch bend and channel pressure messages a MIDI track
/// played in a Session take, placed from the take's start like
/// [`session_notes`]. Polyphonic pressure is in the notes.
#[must_use]
pub fn session_moves(
    messages: &[RecordedMidi],
    start: Frames,
    tempo: &TempoMap,
    rate: SampleRate,
) -> Vec<(Ticks, MidiMessage)> {
    let origin = tempo.frames_to_ticks(start, rate);
    moves(messages, tempo, rate)
        .into_iter()
        .filter(|(_, message)| controller_take::envelope_move(message).is_some())
        .map(|(at, message)| (at - origin, message))
        .collect()
}

/// Writes controller messages into the envelopes of a MIDI clip, as steps
/// (the MIDI controller data spec, R11): `moves` are positions in the clip's
/// contents. With `replace_to`, a controller's envelope is replaced from its
/// first move to there, and what the old one held after it stays (R13, R14); without,
/// the steps are added and the breakpoints already there stay (R15). A
/// controller the clip had no envelope for starts at its resting value.
///
/// # Errors
///
/// [`EditError`] if the document refuses a step.
pub fn write_moves(
    editor: &mut Editor,
    clip: ClipId,
    moves: &[(Ticks, MidiMessage)],
    replace_to: Option<Ticks>,
) -> Result<(), EditError> {
    let mut by_target: BTreeMap<AutomationTarget, Vec<(Ticks, f64)>> = BTreeMap::new();
    for (at, message) in moves {
        if let Some((target, value)) = controller_take::envelope_move(message) {
            by_target.entry(target).or_default().push((*at, value));
        }
    }
    for (target, mut moves) in by_target {
        moves.sort_by_key(|(at, _)| *at);
        let existing = editor
            .document()
            .clip(clip)
            .and_then(|clip| clip.envelope(&target))
            .map(|clip_envelope| clip_envelope.envelope.clone())
            .unwrap_or_default();
        let first = moves[0].0;
        let before = existing.value_at(first - Ticks(1)).unwrap_or(REST);
        let mut written = match replace_to {
            Some(_) => controller_take::stepped(&moves, before),
            None => controller_take::added(&existing.points().collect::<Vec<_>>(), &moves),
        };
        if existing.is_empty() && written[0].0 > Ticks::ZERO {
            written.insert(0, (Ticks::ZERO, before));
        }
        let (mut removed, mut set) = (Vec::new(), written);
        if let Some(end) = replace_to {
            removed = existing
                .points()
                .map(|point| point.at)
                .filter(|at| *at >= set[0].0 && *at < end)
                .collect();
            // An envelope that goes on past the end holds the last step to
            // it, then steps back to where it was.
            let last = *set.last().expect("a move was written");
            if end > last.0 && existing.points().any(|point| point.at >= end) {
                if end > last.0 + Ticks(1) {
                    set.push((end - Ticks(1), last.1));
                }
                set.push((end, existing.value_at(end).unwrap_or(REST)));
            }
        }
        for at in removed {
            editor.apply(Command::DeleteClipEnvelopeBreakpoint {
                clip,
                target,
                kind: EnvelopeKind::Modulation,
                at,
            })?;
        }
        for (at, value) in set {
            editor.apply(Command::SetClipEnvelopeBreakpoint {
                clip,
                target,
                kind: EnvelopeKind::Modulation,
                at,
                value,
            })?;
        }
    }
    Ok(())
}

/// The notes a MIDI track played in a Session take, placed from the take's
/// start: the clip a new recording makes starts there (the session recording
/// spec, R7).
#[must_use]
pub fn session_notes(
    messages: &[RecordedMidi],
    start: Frames,
    end: Frames,
    tempo: &TempoMap,
    rate: SampleRate,
) -> Vec<Note> {
    let origin = tempo.frames_to_ticks(start, rate);
    notes(messages, end, tempo, rate)
        .into_iter()
        .map(|note| Note {
            start: note.start - origin,
            ..note
        })
        .collect()
}

/// Where an overdub puts something played `start` ticks after the clip
/// launched: the pass of the clip's loop it falls in, its place in the
/// contents, and the end of that pass. A looping clip plays its Start marker
/// to the loop's end, then its loop again and again; a clip that doesn't
/// loop plays once, so what is played past its end is `None`.
fn locate(data: &MidiData, length: Ticks, start: Ticks) -> Option<(usize, Ticks, Ticks)> {
    let marker = data.start;
    let (loop_start, loop_end) = (
        data.loop_start,
        data.loop_end
            .unwrap_or_else(|| (data.loop_start + length).max(data.loop_start + Ticks::QUARTER)),
    );
    let (first, rest) = (loop_end - marker, loop_end - loop_start);
    let end = data.end.unwrap_or(marker + length).min(marker + length);
    if start < Ticks::ZERO {
        return None;
    }
    if !data.looping {
        return (marker + start < end).then_some((0, marker + start, end));
    }
    if start < first {
        return Some((0, marker + start, loop_end));
    }
    let past = start - first;
    let pass = 1 + (past.get() / rest.get().max(1)) as usize;
    Some((
        pass,
        loop_start + Ticks(past.get() % rest.get().max(1)),
        loop_end,
    ))
}

/// Sorts the notes an overdub played into the passes of the clip's loop
/// they fall in, placed where the clip plays them (the session recording
/// spec, R8). `played` starts are counted from the frame the clip launched
/// on. A note that would run past the end of its pass is cut there. The map
/// is in order of pass.
#[must_use]
pub fn overdub_passes(
    data: &MidiData,
    length: Ticks,
    played: Vec<Note>,
) -> BTreeMap<usize, Vec<Note>> {
    let mut passes: BTreeMap<usize, Vec<Note>> = BTreeMap::new();
    for note in played {
        let Some((pass, at, limit)) = locate(data, length, note.start) else {
            continue;
        };
        // Spec: mpe R4. A note cut at the end of its pass takes its
        // envelopes' end there.
        let length = note.length.min(limit - at).max(Ticks(1));
        passes.entry(pass).or_default().push(Note {
            start: at,
            ..note.resized(length)
        });
    }
    passes
}

/// Sorts the controller messages an overdub played into the passes of the
/// clip's loop as [`overdub_passes`] does, with the end of each pass, where
/// a controller's replaced envelope resumes (the MIDI controller data spec,
/// R14).
#[must_use]
pub fn overdub_moves(
    data: &MidiData,
    length: Ticks,
    moves: &[(Ticks, MidiMessage)],
) -> BTreeMap<usize, (Ticks, Vec<(Ticks, MidiMessage)>)> {
    let mut passes: BTreeMap<usize, (Ticks, Vec<(Ticks, MidiMessage)>)> = BTreeMap::new();
    for &(start, message) in moves {
        if let Some((pass, at, limit)) = locate(data, length, start) {
            passes
                .entry(pass)
                .or_insert((limit, Vec::new()))
                .1
                .push((at, message));
        }
    }
    passes
}

/// Puts what MIDI tracks recorded into the Arrangement, inside the caller's
/// transaction: a new clip over the take on each track that received notes
/// or controllers, replacing what was there; or, with `overdub` on, each note
/// merged into the MIDI clip it starts in, and the notes that start in no
/// clip making new clips over the gaps between the track's clips, so nothing
/// is replaced (WP-2.14). Controllers go the same way, as steps of the
/// clip's envelopes (WP-2.29b). With `lane` on, each track's take also goes
/// whole on a take lane (the comping spec, R12). `placed` holds the clips
/// already made, which clearing leaves alone.
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
    lane: bool,
    placed: &mut std::collections::BTreeSet<ClipId>,
) -> Result<(), EditError> {
    let tempo = editor.document().settings().tempo.clone();
    let (start, end) = (
        tempo.frames_to_ticks(take.start, rate),
        tempo.frames_to_ticks(take.end, rate),
    );
    for recorded in takes {
        let mut played = notes(&recorded.messages, take.end, &tempo, rate);
        let mut moved = moves(&recorded.messages, &tempo, rate);
        moved.retain(|(_, m)| controller_take::envelope_move(m).is_some());
        if played.is_empty() && moved.is_empty() {
            continue;
        }
        // Spec: comping R12. The take also goes on a take lane of its own,
        // whole, whether or not the main lane merges it.
        if lane {
            let id = editor.new_id();
            let notes = played
                .iter()
                .map(|note| Note {
                    start: note.start - start,
                    ..note.clone()
                })
                .collect::<Vec<_>>();
            let clip = Clip::new(id, start, end - start, ClipData::Midi(MidiData::new(notes)));
            put_on_lane(editor, recorded.track, clip)?;
            // Spec: themes R20. The caller colours what `placed` holds.
            placed.insert(id);
            let steps: Vec<_> = moved.iter().map(|&(at, m)| (at - start, m)).collect();
            write_moves(editor, id, &steps, None)?;
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
            let (steps, rest): (Vec<_>, Vec<_>) =
                moved.into_iter().partition(|(at, _)| at >= from && at < to);
            moved = rest;
            let Some(ClipData::Midi(mut data)) =
                editor.document().clip(*id).map(|clip| clip.data.clone())
            else {
                continue;
            };
            if !inside.is_empty() {
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
            // Spec: midi-controller-data R13. Each controller replaces its
            // envelope from its first message to the take's end.
            let steps: Vec<_> = steps.into_iter().map(|(at, m)| (at - *origin, m)).collect();
            write_moves(editor, *id, &steps, Some(end.min(*to) - *origin))?;
        }
        if played.is_empty() && moved.is_empty() {
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
                    ..note.clone()
                })
                .collect();
            let steps: Vec<_> = moved
                .iter()
                .filter(|(at, _)| *at >= from && *at < to)
                .map(|&(at, m)| (at - from, m))
                .collect();
            if inside.is_empty() && steps.is_empty() {
                continue;
            }
            let id = editor.new_id();
            // Spec: grooves R20. A clip made by recording takes the set's
            // auto-load groove.
            let clip = Clip::new(id, from, to - from, ClipData::Midi(MidiData::new(inside)))
                .with_groove(editor.document().settings().grooves.auto_loaded());
            crate::arrangement::placing::clear(editor, recorded.track, from, to, placed)?;
            placed.insert(id);
            editor.apply(Command::CreateClip {
                track: recorded.track,
                clip,
            })?;
            write_moves(editor, id, &steps, None)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec: recording R12. A take that wrapped is a lead-in and passes.
    #[test]
    fn a_take_that_went_round_the_loop_splits_into_a_lead_in_and_passes() {
        let looped = |start: i64| TakeBounds {
            start: Frames(start),
            end: Frames(0),
            wrap: Some((Frames(100), Frames(200))),
            frames: 0,
        };
        let pass = |song_start: i64, file_from: u64, frames: u64| Pass {
            song_start: Frames(song_start),
            file_from,
            frames,
        };
        // From before the loop: 40 frames of lead-in, two full passes, and
        // 30 frames of a third.
        let (lead, rest) = passes(looped(60), 40 + 100 + 100 + 30);
        assert_eq!(lead, Some(pass(60, 0, 40)));
        assert_eq!(
            rest,
            vec![pass(100, 40, 100), pass(100, 140, 100), pass(100, 240, 30)]
        );
        // From inside the loop: the first pass is partial.
        let (lead, rest) = passes(looped(150), 50 + 100 + 10);
        assert_eq!(lead, None);
        assert_eq!(
            rest,
            vec![pass(150, 0, 50), pass(100, 50, 100), pass(100, 150, 10)]
        );
        // Ending on a pass boundary leaves no empty pass.
        let (_, rest) = passes(looped(100), 200);
        assert_eq!(rest, vec![pass(100, 0, 100), pass(100, 100, 100)]);
    }

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
            Vec::new(),
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
            mpe: MpeRole::Plain,
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
            wrap: None,
            frames: ((to - from) * 96_000) as u64,
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

    // Spec: recording R12. A note held across the loop's wrap ends at the end
    // of the pass it started in, not as a one-tick note: the take's messages
    // carry the frames the take has run, which the wrap does not set back.
    #[test]
    fn a_note_held_across_the_loops_wrap_runs_to_the_end_of_its_pass() {
        let tempo = TempoMap::default();
        let track = TrackId(bl_core::id::Id::new(1).unwrap());
        // A loop of frames 100 to 200, recorded from its start: key 60 goes
        // on 90 frames into the first pass and off 20 frames into the second.
        let bounds = TakeBounds {
            start: Frames(100),
            end: Frames(120),
            wrap: Some((Frames(100), Frames(200))),
            frames: 120,
        };
        let held = played(
            track,
            &[
                (190, MidiMessage::note_on(0, 60, 90)),
                (220, MidiMessage::note_off(0, 60)),
            ],
        );
        let (_, found) = passes(bounds, 120);
        let first = midi_in_pass(&held, bounds, &found[0]);
        let pass = notes(&first[0].messages, Frames(200), &tempo, RATE);
        let [note] = pass.as_slice() else {
            panic!("one note in the first pass: {pass:?}");
        };
        let length = tempo.ticks_to_frames(note.start + note.length, RATE).get()
            - tempo.ticks_to_frames(note.start, RATE).get();
        assert!(
            (9..=11).contains(&length),
            "ten frames to the loop end, not a tick: {length}"
        );
    }

    // The engine still publishes the last recording's take until it has the
    // next one's plan: a recording follows only a take that is not that one,
    // unless the engine has shown the take cleared or running since.
    #[test]
    fn a_recording_ignores_the_take_that_ended_before_it_began() {
        let take = |start: i64, end: i64| TakeBounds {
            start: Frames(start),
            end: Frames(end),
            wrap: None,
            frames: (end - start) as u64,
        };
        let (_plan, mut recording) = start(None, RATE, &[], &[], Vec::new()).unwrap();
        recording.stale = Some(take(0, 100));
        recording.follow(Some(take(0, 100)), false);
        assert_eq!(recording.take, None, "the previous take is not this one's");
        recording.follow(None, false);
        recording.follow(Some(take(50, 80)), false);
        assert_eq!(recording.take, Some(take(50, 80)));
        // A new take that happens to cover the same frames is followed once
        // the engine has been seen to clear the old one.
        let (_plan, mut again) = start(None, RATE, &[], &[], Vec::new()).unwrap();
        again.stale = Some(take(0, 100));
        again.follow(None, true);
        again.follow(Some(take(0, 100)), false);
        assert_eq!(again.take, Some(take(0, 100)));
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
        place_midi(
            &mut editor,
            &takes,
            take(1, 2),
            RATE,
            false,
            false,
            &mut placed,
        )
        .unwrap();

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
        place_midi(
            &mut editor,
            &takes,
            take(0, 2),
            RATE,
            true,
            false,
            &mut placed,
        )
        .unwrap();

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
        place_midi(
            &mut editor,
            &takes,
            take(0, 3),
            RATE,
            true,
            false,
            &mut placed,
        )
        .unwrap();

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
        place_midi(
            &mut editor,
            &takes,
            take(0, 4),
            RATE,
            true,
            false,
            &mut placed,
        )
        .unwrap();

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
        place_midi(
            &mut editor,
            &[],
            take(0, 1),
            RATE,
            false,
            false,
            &mut placed,
        )
        .unwrap();
        assert!(clips(&editor, track).is_empty());
    }

    /// A looping one-bar clip of `looping` set or not, with no notes.
    fn bar_clip(looping: bool) -> MidiData {
        MidiData {
            looping,
            ..MidiData::new([])
        }
    }

    fn notes_at(starts: &[i64]) -> Vec<Note> {
        starts
            .iter()
            .map(|&start| Note::new(60, Ticks(start), Ticks(1_000), 100))
            .collect()
    }

    // Spec: session-recording R8. Notes played over a looping clip fall in
    // the pass of the loop they started in, placed where that pass plays
    // them.
    #[test]
    fn an_overdub_sorts_notes_into_the_loop_passes_they_started_in() {
        let bar = Ticks::WHOLE;
        let passes = overdub_passes(
            &bar_clip(true),
            bar,
            notes_at(&[100, bar.get() + 200, 3 * bar.get() + 300]),
        );
        let shape: Vec<(usize, Vec<i64>)> = passes
            .iter()
            .map(|(pass, notes)| (*pass, notes.iter().map(|n| n.start.get()).collect()))
            .collect();
        assert_eq!(shape, vec![(0, vec![100]), (1, vec![200]), (3, vec![300])]);
    }

    // Spec: session-recording R8. A clip that doesn't loop overdubs until it
    // ends, and a note is cut where its pass ends.
    #[test]
    fn an_overdub_of_a_clip_that_does_not_loop_stops_at_its_end() {
        let bar = Ticks::WHOLE;
        let passes = overdub_passes(&bar_clip(false), bar, notes_at(&[100, bar.get() + 5]));
        assert_eq!(passes.len(), 1);
        assert_eq!(passes[&0].len(), 1);
        let cut = overdub_passes(
            &bar_clip(true),
            bar,
            vec![Note::new(60, Ticks(bar.get() - 10), Ticks(500), 100)],
        );
        assert_eq!(cut[&0][0].length, Ticks(10), "cut at the loop's end");
    }

    // Spec: session-recording R7. A new recording's notes are placed from the
    // take's start, whatever song frame it began on.
    #[test]
    fn a_session_takes_notes_start_from_the_takes_start() {
        let tempo = TempoMap::default();
        let take_start = 96_000;
        let messages = [
            midi(take_start + 12_000, MidiMessage::note_on(0, 60, 100)),
            midi(take_start + 24_000, MidiMessage::note_off(0, 60)),
        ];
        let found = session_notes(
            &messages,
            Frames(take_start),
            Frames(take_start + 48_000),
            &tempo,
            RATE,
        );
        assert_eq!(found.len(), 1);
        assert_eq!(
            tempo.ticks_to_frames(found[0].start, RATE).get(),
            12_000,
            "a quarter second in"
        );
    }

    fn cc(controller: u8, value: u8) -> MidiMessage {
        MidiMessage::from_bytes(&[0xB0, controller, value]).unwrap()
    }

    /// A pitch bend message on `channel` (0 to 15) to `value` (−8192 to 8191).
    fn bend(channel: u8, value: i32) -> MidiMessage {
        let raw = value + 8192;
        MidiMessage::from_bytes(&[0xE0 | channel, (raw & 0x7F) as u8, (raw >> 7) as u8]).unwrap()
    }

    /// The tick a frame of the song is at, 120 BPM.
    fn tick(frame: i64) -> Ticks {
        TempoMap::default().frames_to_ticks(Frames(frame), RATE)
    }

    /// A clip's breakpoints for `target`.
    fn points(editor: &Editor, clip: ClipId, target: AutomationTarget) -> Vec<(Ticks, f64)> {
        editor
            .document()
            .clip(clip)
            .and_then(|clip| clip.envelope(&target))
            .map(|e| e.envelope.points().map(|p| (p.at, p.value)).collect())
            .unwrap_or_default()
    }

    /// The only clip of the track.
    fn only_clip(editor: &Editor, track: TrackId) -> Clip {
        let mut all: Vec<_> = editor
            .document()
            .track(track)
            .unwrap()
            .clips()
            .cloned()
            .collect();
        assert_eq!(all.len(), 1, "{all:?}");
        all.remove(0)
    }

    fn cc1() -> AutomationTarget {
        AutomationTarget::MidiCc { number: 1 }
    }

    // Spec: midi-controller-data R11 (S1, S4). CC1 rising and pitch bend
    // alternating with eight quarter notes, and the resets sent after the
    // last note-off while the take still runs: each controller is a stepped
    // envelope at its resting value before its first message, keeping
    // messages to the end of the take.
    #[test]
    fn a_take_makes_stepped_envelopes_from_the_controllers_played() {
        let (mut editor, track) = editor_with_track();
        let beat = 24_000;
        let mut messages = Vec::new();
        for i in 0..8 {
            let at = i * beat + beat;
            messages.push((at, cc(1, 10 * (i as u8 + 1))));
            messages.push((at, bend(0, if i % 2 == 0 { 4_000 } else { -4_000 })));
            messages.push((at, MidiMessage::note_on(0, 60, 100)));
            messages.push((at + 6_000, MidiMessage::note_off(0, 60)));
        }
        // After the last note-off, with the take still running.
        messages.push((9 * beat + 12_000, cc(1, 0)));
        messages.push((9 * beat + 12_000, bend(0, 0)));
        messages.sort_by_key(|(at, _)| *at);
        let takes = played(track, &messages);
        let mut placed = Default::default();
        place_midi(
            &mut editor,
            &takes,
            take(0, 3),
            RATE,
            false,
            false,
            &mut placed,
        )
        .unwrap();

        let clip = only_clip(&editor, track);
        let one = |frame, value| (tick(frame), value);
        let hold = |frame, value| (tick(frame) - Ticks(1), value);
        let mut expected = vec![(Ticks::ZERO, 0.0), hold(beat, 0.0)];
        for i in 0..8 {
            let at = (i + 1) * beat;
            if i > 0 {
                expected.push(hold(at, 10.0 * i as f64));
            }
            expected.push(one(at, 10.0 * (i as f64 + 1.0)));
        }
        expected.push(hold(9 * beat + 12_000, 80.0));
        expected.push(one(9 * beat + 12_000, 0.0));
        assert_eq!(points(&editor, clip.id, cc1()), expected);
        let bends = points(&editor, clip.id, AutomationTarget::MidiPitchBend);
        assert_eq!(
            bends[0],
            (Ticks::ZERO, 0.0),
            "centre before the first message"
        );
        assert_eq!(bends[2], one(beat, 4_000.0));
        assert_eq!(bends[4], one(2 * beat, -4_000.0));
        assert_eq!(*bends.last().unwrap(), one(9 * beat + 12_000, 0.0));
        assert!(
            clip.envelope(&AutomationTarget::MidiChannelPressure)
                .is_none()
        );
    }

    // Spec: midi-controller-data R11 (S3). The same controller on two
    // channels on one frame makes one envelope holding the later message.
    #[test]
    fn the_later_of_two_messages_on_one_frame_wins_whatever_the_channel() {
        let (mut editor, track) = editor_with_track();
        let on_one = MidiMessage::from_bytes(&[0xB0, 1, 30]).unwrap();
        let on_two = MidiMessage::from_bytes(&[0xB1, 1, 90]).unwrap();
        let takes = played(
            track,
            &[
                (24_000, MidiMessage::note_on(0, 60, 100)),
                (24_000, on_one),
                (24_000, on_two),
                (30_000, MidiMessage::note_off(0, 60)),
            ],
        );
        let mut placed = Default::default();
        place_midi(
            &mut editor,
            &takes,
            take(0, 1),
            RATE,
            false,
            false,
            &mut placed,
        )
        .unwrap();
        let clip = only_clip(&editor, track);
        assert_eq!(clip.envelopes().len(), 1, "one envelope, no channel");
        assert_eq!(
            points(&editor, clip.id, cc1()).last(),
            Some(&(tick(24_000), 90.0))
        );
    }

    // Spec: midi-controller-data R12 (S4). A take that received only a
    // controller still makes a clip over the whole take, with no notes.
    #[test]
    fn a_controller_only_take_still_makes_a_clip() {
        let (mut editor, track) = editor_with_track();
        add_clip(&mut editor, track, 0, 4, 40);
        let takes = played(track, &[(96_000 + 24_000, cc(1, 77))]);
        let mut placed = Default::default();
        place_midi(
            &mut editor,
            &takes,
            take(1, 2),
            RATE,
            false,
            false,
            &mut placed,
        )
        .unwrap();
        // The old clip keeps bar 0 and bars 2 to 3; the take is bar 1.
        assert_eq!(
            clips(&editor, track),
            vec![(0, 1, vec![40]), (1, 1, vec![]), (2, 2, vec![40])]
        );
        let clip = editor
            .document()
            .track(track)
            .unwrap()
            .clips()
            .find(|clip| clip.start == Ticks::WHOLE)
            .unwrap();
        assert_eq!(
            points(&editor, clip.id, cc1()).last(),
            Some(&(tick(24_000), 77.0))
        );
    }

    // Spec: midi-controller-data R11 (S1); mpe R12. Polyphonic pressure is the
    // held note's own pressure, stepped, and makes no envelope; with no note held it is
    // left out, and a take of nothing else makes no clip.
    #[test]
    fn polyphonic_pressure_becomes_the_pressure_of_the_note_held() {
        let (mut editor, track) = editor_with_track();
        let pressure = |key, value| MidiMessage::from_bytes(&[0xA0, key, value]).unwrap();
        let takes = played(
            track,
            &[
                (5_000, pressure(60, 9)),
                (24_000, MidiMessage::note_on(0, 60, 100)),
                (30_000, pressure(60, 127)),
                (36_000, pressure(60, 112)),
                (40_000, pressure(61, 1)),
                (48_000, MidiMessage::note_off(0, 60)),
            ],
        );
        let mut placed = Default::default();
        place_midi(
            &mut editor,
            &takes,
            take(0, 1),
            RATE,
            false,
            false,
            &mut placed,
        )
        .unwrap();
        let clip = only_clip(&editor, track);
        let ClipData::Midi(data) = &clip.data else {
            panic!("a MIDI clip")
        };
        let note = data.notes.iter().next().unwrap();
        assert_eq!(
            note.pressure
                .iter()
                .map(|p| (p.at, p.value))
                .collect::<Vec<_>>(),
            vec![
                (Ticks(0), 0.0),
                (tick(30_000) - tick(24_000) - Ticks(1), 0.0),
                (tick(30_000) - tick(24_000), 127.0),
                (tick(36_000) - tick(24_000) - Ticks(1), 127.0),
                (tick(36_000) - tick(24_000), 112.0)
            ]
        );
        assert_eq!(clip.envelopes().len(), 0);

        let (mut editor, track) = editor_with_track();
        let takes = played(track, &[(5_000, pressure(60, 9))]);
        place_midi(
            &mut editor,
            &takes,
            take(0, 1),
            RATE,
            false,
            false,
            &mut Default::default(),
        )
        .unwrap();
        assert!(clips(&editor, track).is_empty());
    }

    /// A one-bar looping clip over bar 0 with a CC1 envelope that steps from
    /// 10 to 20 half way.
    fn clip_with_cc1(editor: &mut Editor, track: TrackId) -> ClipId {
        let id = editor.new_id();
        let data = MidiData {
            looping: true,
            ..MidiData::new([])
        };
        editor
            .apply(Command::CreateClip {
                track,
                clip: Clip::new(id, Ticks::ZERO, Ticks::WHOLE, ClipData::Midi(data)),
            })
            .unwrap();
        let half = tick(48_000);
        for (at, value) in [(Ticks::ZERO, 10.0), (half - Ticks(1), 10.0), (half, 20.0)] {
            editor
                .apply(Command::SetClipEnvelopeBreakpoint {
                    clip: id,
                    target: cc1(),
                    kind: EnvelopeKind::Modulation,
                    at,
                    value,
                })
                .unwrap();
        }
        id
    }

    // Spec: midi-controller-data R13 (S5). With MIDI Arrangement Overdub on,
    // CC1 = 127 replaces that controller's envelope from its first message
    // to the take's end, keeping its earlier breakpoints; CC2, which the
    // clip didn't have, starts at its resting value, and the clip's notes
    // stay.
    #[test]
    fn arrangement_overdub_replaces_a_controller_from_its_first_message() {
        let (mut editor, track) = editor_with_track();
        let id = clip_with_cc1(&mut editor, track);
        let takes = played(
            track,
            &[
                (60_000, cc(1, 127)),
                (70_000, cc(2, 20)),
                (80_000, cc(1, 127)),
            ],
        );
        let mut placed = Default::default();
        place_midi(
            &mut editor,
            &takes,
            take(0, 1),
            RATE,
            true,
            false,
            &mut placed,
        )
        .unwrap();

        let ids: Vec<_> = editor
            .document()
            .track(track)
            .unwrap()
            .clips()
            .map(|c| c.id)
            .collect();
        assert_eq!(ids, vec![id], "the same clip, no new one");
        assert_eq!(
            points(&editor, id, cc1()),
            vec![
                (Ticks::ZERO, 10.0),
                (tick(48_000) - Ticks(1), 10.0),
                (tick(48_000), 20.0),
                (tick(60_000) - Ticks(1), 20.0),
                (tick(60_000), 127.0),
                (tick(80_000) - Ticks(1), 127.0),
                (tick(80_000), 127.0),
            ],
            "the old step after the first message is gone"
        );
        assert_eq!(
            points(&editor, id, AutomationTarget::MidiCc { number: 2 }),
            vec![
                (Ticks::ZERO, 0.0),
                (tick(70_000) - Ticks(1), 0.0),
                (tick(70_000), 20.0),
            ]
        );
    }

    // Spec: midi-controller-data R14 (S5). A Session overdub replaces each
    // controller from its first message to the end of the loop, in the pass
    // it was played in, keeping the breakpoints before it.
    #[test]
    fn a_session_overdub_replaces_a_controller_to_the_end_of_the_loop() {
        let (mut editor, track) = editor_with_track();
        let id = clip_with_cc1(&mut editor, track);
        let Some(ClipData::Midi(data)) = editor.document().clip(id).map(|c| c.data.clone()) else {
            panic!("a MIDI clip")
        };
        // Played 60,000 frames after the clip launched, and again 96,000 later.
        let moves = [
            (tick(60_000), cc(1, 127)),
            (tick(96_000 + 70_000), cc(1, 5)),
        ];
        let passes = overdub_moves(&data, Ticks::WHOLE, &moves);
        assert_eq!(passes.keys().copied().collect::<Vec<_>>(), vec![0, 1]);
        let (end, first) = &passes[&0];
        assert_eq!(*end, Ticks::WHOLE);
        write_moves(&mut editor, id, first, Some(*end)).unwrap();
        assert_eq!(
            points(&editor, id, cc1()),
            vec![
                (Ticks::ZERO, 10.0),
                (tick(48_000) - Ticks(1), 10.0),
                (tick(48_000), 20.0),
                (tick(60_000) - Ticks(1), 20.0),
                (tick(60_000), 127.0),
            ],
            "127 holds to the end of the loop"
        );
    }

    // Spec: midi-controller-data R15 (S2). Controllers captured into a clip
    // that plays are added as steps, and the clip's breakpoints stay.
    #[test]
    fn captured_controllers_are_added_and_the_old_breakpoints_stay() {
        let (mut editor, track) = editor_with_track();
        let id = clip_with_cc1(&mut editor, track);
        write_moves(
            &mut editor,
            id,
            &[(tick(24_000), cc(1, 127)), (tick(60_000), cc(1, 127))],
            None,
        )
        .unwrap();
        assert_eq!(
            points(&editor, id, cc1()),
            vec![
                (Ticks::ZERO, 10.0),
                (tick(24_000) - Ticks(1), 10.0),
                (tick(24_000), 127.0),
                (tick(48_000) - Ticks(2), 127.0),
                (tick(48_000) - Ticks(1), 10.0),
                (tick(48_000), 20.0),
                (tick(60_000) - Ticks(1), 20.0),
                (tick(60_000), 127.0),
            ]
        );
    }
}
