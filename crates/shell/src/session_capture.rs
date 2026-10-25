//! Recording the Session into the Arrangement (WP-3.13).
//!
//! While an Arrangement take runs, the engine logs every Session clip that
//! starts and ends on its song frame ([`SessionEvent`]). When the take ends,
//! [`runs`] pairs those into the stretches each clip played, and [`place`]
//! copies each stretch into its track's Arrangement as a clip of the same
//! length, with the Session clip's loop settings and a start marker that
//! stands where the clip was in its loop. [`Watch`] keeps what the Session
//! clips were while they played, so a clip edited mid-take is copied as it
//! was before the edit and again, from the edit on, as it became.

use std::collections::{BTreeMap, BTreeSet};

use bl_core::time::{Frames, SampleRate, TempoMap, Ticks};
use bl_engine::{SessionEvent, SlotState, SlotStatus};
use bl_model::{Clip, ClipData, ClipId, ClipLaunch, Command, Document, EditError, Editor};
use bl_model::{SceneId, TrackId};

/// A slot: the track and the scene whose clip it holds.
type Slot = (TrackId, SceneId);

/// One stretch a Session clip played during a take, in ticks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    /// The slot that played.
    pub slot: Slot,
    /// Where the stretch begins.
    pub from: Ticks,
    /// Where it ends.
    pub to: Ticks,
    /// Where the clip launched, which is `from` unless the clip was already
    /// playing when the take began.
    pub launched: Ticks,
    /// The clip time it had where it launched: not zero for a Legato start
    /// or after a nudge, which splits a stretch (launch-quantisation R33).
    pub offset: Ticks,
}

/// Pairs a take's starts and ends into the stretches the clips played: a
/// restart ends one and starts the next on a frame, and a clip still
/// playing when the take ends at `end` plays up to it.
#[must_use]
pub fn runs(events: &[SessionEvent], end: Frames, tempo: &TempoMap, rate: SampleRate) -> Vec<Run> {
    let mut open: BTreeMap<Slot, (Frames, Frames, Ticks)> = BTreeMap::new();
    let mut played = Vec::new();
    for event in events {
        let slot = (event.track, event.scene);
        if let Some((from, launched, offset)) = open.remove(&slot) {
            played.push((slot, from, event.at, launched, offset));
        }
        if let Some(launched) = event.launched {
            open.insert(slot, (event.at, launched, event.offset.ticks));
        }
    }
    played.extend(
        open.into_iter()
            .map(|(slot, (from, launched, offset))| (slot, from, end, launched, offset)),
    );
    let ticks = |frame: Frames| tempo.frames_to_ticks(frame, rate);
    let mut runs: Vec<Run> = played
        .into_iter()
        .filter_map(|(slot, from, to, launched, offset)| {
            let to = to.min(end);
            (to > from).then(|| Run {
                slot,
                from: ticks(from),
                to: ticks(to),
                launched: ticks(launched),
                offset,
            })
        })
        .collect();
    runs.sort_by_key(|run| (run.slot, run.from));
    runs
}

/// What the Session clips that play were, from the moment they were seen to
/// play or to change: the source of the copies.
#[derive(Default)]
pub struct Watch {
    /// A playing slot's clip from `Ticks` on, in the order seen. The launch
    /// settings are left out: they aren't written (the spec's R1).
    states: Vec<(Slot, Ticks, Clip)>,
    /// The frame each slot seen playing launched on, so a relaunch starts a
    /// new state.
    seen: BTreeMap<Slot, Frames>,
}

impl Watch {
    /// Notes the clips in the slots that play now, at the playhead
    /// `position`: a slot seen for the first time, or launched again, is
    /// noted from its launch, and one whose clip differs from the last noted
    /// is noted from here.
    pub fn poll(
        &mut self,
        document: &Document,
        slots: &[SlotStatus],
        position: Ticks,
        tempo: &TempoMap,
        rate: SampleRate,
    ) {
        let playing = |status: &&SlotStatus| {
            matches!(status.state, SlotState::Playing | SlotState::QueuedToStop)
        };
        self.seen.retain(|slot, _| {
            slots
                .iter()
                .filter(playing)
                .any(|status| (status.track, status.scene) == *slot)
        });
        for status in slots.iter().filter(playing) {
            let (slot, Some(started)) = ((status.track, status.scene), status.started) else {
                continue;
            };
            let Some(clip) = document.track(slot.0).and_then(|track| track.slot(slot.1)) else {
                continue;
            };
            let clip = Clip {
                launch: ClipLaunch::default(),
                ..clip.clone()
            };
            let relaunched = self.seen.insert(slot, started) != Some(started);
            let last = self.states.iter().rev().find(|(noted, ..)| *noted == slot);
            if relaunched {
                let from = tempo.frames_to_ticks(started, rate);
                self.states.push((slot, from, clip));
            } else if last.is_none_or(|(_, _, noted)| *noted != clip) {
                self.states.push((slot, position, clip));
            }
        }
    }

    /// The clip `slot` played at `at`: the last noted from then or before,
    /// or the first noted.
    fn clip_at(&self, slot: Slot, at: Ticks) -> Option<&Clip> {
        let mut noted = self.states.iter().filter(|(s, ..)| *s == slot);
        let first = noted.next()?;
        Some(
            &noted
                .take_while(|(_, from, _)| *from <= at)
                .last()
                .unwrap_or(first)
                .2,
        )
    }
}

/// Copies each run into its track's Arrangement, replacing what lies under
/// it, as the one transaction the caller has open. A run is cut where its
/// clip was edited during it. An armed track, whose input the take recorded
/// too, keeps that recording only up to where its first copy begins.
///
/// # Errors
///
/// [`EditError`] if the document refuses a step; the caller's transaction
/// then puts everything back.
pub fn place(
    editor: &mut Editor,
    watch: &Watch,
    runs: &[Run],
    armed: &[TrackId],
    end: Ticks,
) -> Result<(), EditError> {
    let tempo = editor.document().settings().tempo.clone();
    let mut placed = BTreeSet::<ClipId>::new();
    let mut first: BTreeMap<TrackId, (Ticks, BTreeSet<ClipId>)> = BTreeMap::new();
    for run in runs {
        let cuts = watch
            .states
            .iter()
            .filter(|(slot, at, _)| *slot == run.slot && run.from < *at && *at < run.to)
            .map(|(_, at, _)| *at);
        let bounds: Vec<Ticks> = std::iter::once(run.from)
            .chain(cuts)
            .chain(std::iter::once(run.to))
            .collect();
        for span in bounds.windows(2) {
            let (from, to) = (span[0], span[1]);
            let source = watch.clip_at(run.slot, from).cloned().or_else(|| {
                let (track, scene) = run.slot;
                let slot = editor.document().track(track)?.slot(scene)?;
                Some(slot.clone())
            });
            let Some(source) = source else {
                continue;
            };
            // A clip that took over another's time, or was nudged, played as
            // if it had launched its offset earlier (launch-quantisation R33).
            let launched = run.launched - run.offset;
            let data = match source.data {
                ClipData::Midi(midi) => ClipData::Midi(midi.moved_by(launched, from)),
                ClipData::Audio(audio) => ClipData::Audio(audio.moved_by(launched, from, &tempo)),
            };
            let id = editor.new_id();
            let mut clip = Clip::new(id, from, to - from, data);
            clip.name = source.name;
            clip.mute = source.mute;
            clip.envelopes = source.envelopes;
            // Spec: grooves R3. A copy keeps its groove.
            clip.groove = source.groove;
            // Spec: themes R20. A copy keeps its colour.
            clip.colour = source.colour;
            let track = run.slot.0;
            crate::arrangement::placing::clear(editor, track, from, to, &placed)?;
            placed.insert(id);
            let entry = first.entry(track).or_insert((from, BTreeSet::new()));
            entry.0 = entry.0.min(from);
            entry.1.insert(id);
            editor.apply(Command::CreateClip { track, clip })?;
        }
    }
    for (track, (from, copies)) in first {
        if armed.contains(&track) {
            crate::arrangement::placing::clear(editor, track, from, end, &copies)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use bl_core::time::Bpm;
    use bl_engine::{EngineConfig, MediaMap, Offline, Registry, SessionCommand, TransportCommand};
    use bl_media::SampleData;
    use bl_model::{AudioData, ContentHash, TrackKind};

    use super::*;

    const RATE: SampleRate = SampleRate::HZ_48000;
    /// A bar at 120 BPM and 48 kHz, in frames.
    const BAR: i64 = 96_000;
    const HASH: ContentHash = ContentHash([7; 32]);

    fn tempo() -> TempoMap {
        TempoMap::constant(Bpm::new(120.0).unwrap())
    }

    fn ticks(frame: i64) -> Ticks {
        tempo().frames_to_ticks(Frames(frame), RATE)
    }

    /// A bar-long file whose every sample says where it is.
    fn ramp() -> Arc<SampleData> {
        let side = |sign: f32| {
            (0..BAR as usize)
                .map(|i| sign * (i + 1) as f32 / 65_536.0)
                .collect()
        };
        Arc::new(SampleData::new(48_000, vec![side(1.0), side(-1.0)]).unwrap())
    }

    /// A set whose only track holds a bar-long audio clip in a Session
    /// slot, looping over the first half second of the ramp.
    fn set() -> (Editor, TrackId, SceneId, ClipId) {
        let mut editor = Editor::new();
        let track = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id: track,
                kind: TrackKind::Audio,
                name: String::new(),
                parent: None,
                index: 0,
            })
            .unwrap();
        let scene = editor.document().scenes().next().unwrap().id;
        let audio = AudioData {
            looping: true,
            loop_end: Some(0.5),
            ..AudioData::new(HASH)
        };
        let id = editor.new_id();
        let clip = Clip::new(id, Ticks::ZERO, Ticks::WHOLE, ClipData::Audio(audio));
        editor
            .apply(Command::CreateSlotClip { track, scene, clip })
            .unwrap();
        (editor, track, scene, id)
    }

    type Cue = Box<dyn FnOnce(&mut Offline, &mut Editor)>;

    /// The left channel of the set rendered to frame `until`, with each cue
    /// run on its frame.
    fn render(editor: &mut Editor, until: i64, mut cues: Vec<(i64, Cue)>) -> Vec<f32> {
        let mut media = MediaMap::new();
        media.insert(HASH, ramp());
        let registry = Registry::new().with_media(Arc::new(media));
        let mut offline = Offline::new(EngineConfig::new(RATE, 512), registry);
        offline.sync(editor.document());
        cues.sort_by_key(|(frame, _)| *frame);
        let mut cues = cues.into_iter().peekable();
        let mut out = Vec::new();
        while (out.len() as i64) < until {
            let done = out.len() as i64;
            while let Some((_, cue)) = cues.next_if(|(frame, _)| *frame <= done) {
                cue(&mut offline, editor);
            }
            let next = cues.peek().map_or(until, |(frame, _)| *frame);
            let frames = 512.min(until - done).min(next - done);
            offline.render(frames as u64, |output| {
                out.extend_from_slice(output.channel(0));
            });
        }
        out
    }

    fn start(track: TrackId, scene: SceneId, at: i64, launched: i64) -> SessionEvent {
        SessionEvent {
            track,
            scene,
            at: Frames(at),
            launched: Some(Frames(launched)),
            offset: bl_engine::ClipOffset::ZERO,
        }
    }

    fn end(track: TrackId, scene: SceneId, at: i64) -> SessionEvent {
        SessionEvent {
            track,
            scene,
            at: Frames(at),
            launched: None,
            offset: bl_engine::ClipOffset::ZERO,
        }
    }

    // Spec: session-to-arrangement R1, R2. A restart ends one stretch and
    // starts the next on a frame, a clip playing when the take ends plays to
    // its end, and a stretch is as long as the clip played.
    #[test]
    fn starts_and_ends_pair_into_the_stretches_the_clips_played() {
        let (_, track, scene, _) = set();
        let events = [
            start(track, scene, 10 * 29, 10 * 29 - 100),
            start(track, scene, BAR, BAR),
            end(track, scene, 2 * BAR),
            start(track, scene, 3 * BAR, 3 * BAR),
        ];
        let found = runs(&events, Frames(4 * BAR), &tempo(), RATE);
        let spans: Vec<_> = found.iter().map(|run| (run.from, run.to)).collect();
        assert_eq!(
            spans,
            vec![
                (ticks(290), ticks(BAR)),
                (ticks(BAR), ticks(2 * BAR)),
                (ticks(3 * BAR), ticks(4 * BAR)),
            ]
        );
        assert_eq!(found[0].launched, ticks(190));
    }

    // Spec: session-to-arrangement R1, R2, R3. The Arrangement made of the
    // copies plays what the Session performance played, to the sample: a
    // clip already playing when the take began, stopped, launched again, and
    // edited in the middle of a stretch.
    #[test]
    fn the_copies_render_as_the_performance_did() {
        let (mut editor, track, scene, clip) = set();
        let launch = SessionCommand::Launch { track, scene };
        let play: Cue = Box::new(|offline, _| offline.sender().transport(TransportCommand::Play));
        let cue = |command: SessionCommand| -> Cue {
            Box::new(move |offline, _| offline.sender().session(command))
        };
        // The edit: a loop a quarter second long from here on.
        let edit: Cue = Box::new(move |offline, editor| {
            editor
                .apply(Command::SetClipLoop {
                    clip,
                    start: 0.0,
                    end: Some(0.25),
                    ticks: false,
                })
                .unwrap();
            offline.sync(editor.document());
        });
        let performance = render(
            &mut editor,
            4 * BAR,
            vec![
                (0, play),
                (1, cue(launch)),
                (132_000, edit),
                (150_000, cue(SessionCommand::StopTrack(track))),
                (200_000, cue(launch)),
            ],
        );

        // What the engine logs for the take that began at frame 126_000,
        // and the shell's watch of the clip's two states.
        let slot = &[bl_engine::SlotStatus {
            track,
            scene,
            state: SlotState::Playing,
            started: Some(Frames(BAR)),
            offset: bl_engine::ClipOffset::ZERO,
        }];
        // A second set has the same IDs, and the clip as it was.
        let (before, same, ..) = set();
        assert_eq!(same, track);
        let mut watch = Watch::default();
        watch.poll(before.document(), slot, ticks(126_000), &tempo(), RATE);
        watch.poll(editor.document(), slot, ticks(132_000), &tempo(), RATE);
        let events = [
            start(track, scene, 126_000, BAR),
            end(track, scene, 2 * BAR),
            start(track, scene, 3 * BAR, 3 * BAR),
        ];
        let found = runs(&events, Frames(4 * BAR), &tempo(), RATE);
        let (mut target, ..) = set();
        target
            .transact("Record", |editor| {
                place(editor, &watch, &found, &[], Ticks::WHOLE * 4)
            })
            .unwrap();
        let copies: Vec<_> = target
            .document()
            .track(track)
            .unwrap()
            .clips()
            .map(|clip| (clip.start, clip.end()))
            .collect();
        assert_eq!(
            copies,
            vec![
                (ticks(126_000), ticks(132_000)),
                (ticks(132_000), ticks(2 * BAR)),
                (ticks(3 * BAR), ticks(4 * BAR)),
            ]
        );
        let played = render(
            &mut target,
            4 * BAR,
            vec![(
                0,
                Box::new(|offline, _| offline.sender().transport(TransportCommand::Play)),
            )],
        );
        for (from, to) in [(126_000, 2 * BAR), (3 * BAR, 4 * BAR)] {
            let range = from as usize..to as usize;
            assert!(performance[range.clone()].iter().any(|&s| s != 0.0));
            assert_eq!(played[range.clone()], performance[range], "{from}..{to}");
        }
        assert!(played[..126_000].iter().all(|&s| s == 0.0));
        assert!(
            played[2 * BAR as usize..3 * BAR as usize]
                .iter()
                .all(|&s| s == 0.0)
        );
    }

    // Spec: session-to-arrangement R4. An armed track's recorded input is
    // kept up to where its first copy begins; any other track keeps its
    // older clips beyond the copy.
    #[test]
    fn an_armed_tracks_input_is_kept_only_up_to_its_first_copy() {
        let tail = |armed: bool| {
            let (mut editor, track, scene, _) = set();
            let id = editor.new_id();
            let input = Clip::new(
                id,
                Ticks::ZERO,
                Ticks::WHOLE * 4,
                ClipData::Audio(AudioData::new(HASH)),
            );
            editor
                .apply(Command::CreateClip { track, clip: input })
                .unwrap();
            let events = [start(track, scene, BAR, BAR), end(track, scene, 2 * BAR)];
            let found = runs(&events, Frames(4 * BAR), &tempo(), RATE);
            let armed: &[TrackId] = if armed { &[track] } else { &[] };
            place(
                &mut editor,
                &Watch::default(),
                &found,
                armed,
                ticks(4 * BAR),
            )
            .unwrap();
            let mut spans: Vec<_> = editor
                .document()
                .track(track)
                .unwrap()
                .clips()
                .map(|clip| (clip.start, clip.end()))
                .collect();
            spans.sort();
            spans
        };
        let copy = (ticks(BAR), ticks(2 * BAR));
        assert_eq!(tail(true), vec![(Ticks::ZERO, ticks(BAR)), copy]);
        assert_eq!(
            tail(false),
            vec![
                (Ticks::ZERO, ticks(BAR)),
                copy,
                (ticks(2 * BAR), ticks(4 * BAR))
            ]
        );
    }
}
