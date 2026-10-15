//! Session View launching, in the engine (WP-3.2): one state machine per
//! clip slot, the launch quantisation grid, and what a track's player needs
//! to play a launched clip.
//!
//! A launch or a stop asked for while the song plays lands on the exact
//! frame of the global launch quantisation grid (the launch quantisation
//! spec): [`SessionLauncher::launch`] and [`SessionLauncher::stop`] work out
//! that frame once, in [`SessionLauncher::frame_for`], and the processor
//! splits its block there, as it does for a transport command. While the
//! song is stopped a launch takes effect at once, and the processor starts
//! the transport from the Arrangement position.
//!
//! The engine publishes each slot's state after every block
//! ([`SlotStatus`]) for the UI to draw, and hands every track's player the
//! clip playing on it ([`SessionPlayback`]) so the player sounds the
//! launched clip from its launch frame.

use bl_core::time::{Frames, SampleRate, TempoMap, Ticks, TimeSignatureMap};
use bl_model::{LaunchSettings, SceneId, TrackId};

/// What a slot is doing (WP-3.2). Recording waits for WP-3.11.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotState {
    /// A launch or a stop is waiting for the grid.
    Queued,
    /// The slot's clip plays.
    Playing,
    /// A stop is waiting for the grid.
    QueuedToStop,
}

/// One slot's state, as the engine publishes it for the UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotStatus {
    /// The track whose slot it is.
    pub track: TrackId,
    /// The scene whose slot it is.
    pub scene: SceneId,
    /// What it's doing.
    pub state: SlotState,
}

/// A slot's clip playing on a track, as its player needs it: which slot,
/// and the song frame the clip launched on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionPlayback {
    /// The track playing it.
    pub track: TrackId,
    /// The scene whose slot clip plays.
    pub scene: SceneId,
    /// The song frame the clip launched on.
    pub frame: Frames,
}

/// A command to the engine's Session View (WP-3.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionCommand {
    /// Launches the clip in a slot: the next grid line while the song plays,
    /// at once while it's stopped (and the transport starts).
    Launch {
        /// The track whose slot it is.
        track: TrackId,
        /// The scene whose slot it is.
        scene: SceneId,
    },
    /// Launches every clip of a scene, all on one grid line.
    LaunchScene(SceneId),
    /// Stops a track's playing clip on the next grid line.
    StopTrack(TrackId),
    /// Stops every track's playing clip on the next grid line.
    StopAll,
    /// Sets the global launch quantisation and its switch.
    SetQuantization(LaunchSettings),
}

/// The transport as the launcher reads it: where the playhead is, whether
/// the song plays, and the maps that place ticks on frames.
#[derive(Clone, Copy)]
pub(crate) struct Playhead<'a> {
    /// Whether a launch or a stop waits for the grid: it does while the song
    /// plays, and takes effect at once while it doesn't (the launch
    /// quantisation spec, R3 and R5).
    pub(crate) wait_for_grid: bool,
    /// The playhead in ticks.
    pub(crate) position: Ticks,
    /// The running tempo map.
    pub(crate) tempo: &'a TempoMap,
    /// The running time-signature map, which places the bar lines.
    pub(crate) signatures: &'a TimeSignatureMap,
    /// The engine's rate.
    pub(crate) rate: SampleRate,
}

impl Playhead<'_> {
    /// The song frame a position in ticks falls on.
    fn frames(&self, at: Ticks) -> Frames {
        self.tempo.ticks_to_frames(at.max(Ticks::ZERO), self.rate)
    }
}

/// One slot the launcher knows about: it plays, or it changes at a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Slot {
    track: TrackId,
    scene: SceneId,
    /// The song frame the clip plays from, while it plays.
    started: Option<Frames>,
    /// The frame it changes at: where a queued launch plays from, or where
    /// a queued stop lands.
    due: Option<Frames>,
}

impl Slot {
    /// What the slot is doing, for the UI.
    fn state(self) -> SlotState {
        match (self.started, self.due) {
            (Some(_), Some(_)) => SlotState::QueuedToStop,
            (Some(_), None) => SlotState::Playing,
            _ => SlotState::Queued,
        }
    }
}

/// Every slot's state machine, and the global launch quantisation.
///
/// A stopped slot isn't kept: the list holds only the slots that are queued,
/// playing, or queued to stop, so it stays small. A track holds at most two
/// of them: the clip playing (or stopping) and one queued to replace it, so
/// the capacity is twice the engine's track ceiling.
pub(crate) struct SessionLauncher {
    settings: LaunchSettings,
    slots: Vec<Slot>,
    /// Launches dropped because every slot was taken.
    pub(crate) overflowed: u64,
}

impl SessionLauncher {
    /// A launcher with the default quantisation, with room for `capacity`
    /// slots.
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            settings: LaunchSettings::default(),
            slots: Vec::with_capacity(capacity),
            overflowed: 0,
        }
    }

    /// The global launch quantisation and its switch.
    pub(crate) fn settings(&self) -> LaunchSettings {
        self.settings
    }

    /// Sets the quantisation and the switch. A launch already queued keeps
    /// the frame it was queued for (the launch quantisation spec, R4).
    pub(crate) fn set_settings(&mut self, settings: LaunchSettings) {
        self.settings = settings;
    }

    /// Forgets every slot: a processor that takes over after a device
    /// restart starts with a Session View at rest, as with the song stopped
    /// (device-changes R7).
    pub(crate) fn clear(&mut self) {
        self.slots.clear();
    }

    /// The frame a launch or a stop asked for now waits for: the next line
    /// of the quantisation grid, at once while the song is stopped or
    /// quantisation is off. The one place the grid is worked out.
    fn frame_for(&self, playhead: Playhead<'_>) -> Frames {
        if !playhead.wait_for_grid {
            return playhead.frames(playhead.position);
        }
        playhead.frames(self.settings.next_line(playhead.position, playhead.signatures))
    }

    /// Launches the clip in `(track, scene)`. While the song plays it plays
    /// from the next grid line; while it's stopped, the caller starts the
    /// transport and the clip plays from the playhead at once.
    pub(crate) fn launch(&mut self, track: TrackId, scene: SceneId, playhead: Playhead<'_>) {
        let at = self.frame_for(playhead);
        self.launch_at(track, scene, at);
    }

    /// Launches every slot of `scene` that `slots` names (`(scene, track)`
    /// pairs, from the running program), all on one frame: a scene's clips
    /// start together (observed in Live 12.4.3).
    pub(crate) fn launch_scene(
        &mut self,
        scene: SceneId,
        slots: &[(SceneId, TrackId)],
        playhead: Playhead<'_>,
    ) {
        let at = self.frame_for(playhead);
        for &(_, track) in slots {
            self.launch_at(track, scene, at);
        }
    }

    /// Launches `(track, scene)` to play from `at` (already worked out, so
    /// a scene's slots share one frame).
    fn launch_at(&mut self, track: TrackId, scene: SceneId, at: Frames) {
        // Already playing: a second launch of the same slot is the launch
        // modes' business (WP-3.3).
        if self
            .find_mut(track, scene)
            .is_some_and(|slot| slot.started.is_some())
        {
            return;
        }
        self.clear_way_for(track, scene, at);
        if self.slots.len() == self.slots.capacity() {
            self.overflowed += 1;
            return;
        }
        self.slots.push(Slot {
            track,
            scene,
            started: None,
            due: Some(at),
        });
    }

    /// Makes the track's other slots give way at `at`: the clip playing
    /// stops there (it plays until then), and a launch still queued is
    /// dropped, because the later launch wins (the acceptance's "two
    /// launches on one track act in order").
    fn clear_way_for(&mut self, track: TrackId, except: SceneId, at: Frames) {
        for slot in &mut self.slots {
            if slot.track != track || slot.scene == except {
                continue;
            }
            match slot.started {
                Some(_) => slot.due = Some(at),
                None => slot.due = None,
            }
        }
        self.slots
            .retain(|slot| slot.track != track || slot.started.is_some());
    }

    /// Stops the playing clip of `track`, or of every track with `None`, on
    /// the next grid line. A launch still queued on the track is dropped.
    pub(crate) fn stop(&mut self, track: Option<TrackId>, playhead: Playhead<'_>) {
        let at = self.frame_for(playhead);
        for slot in &mut self.slots {
            if track.is_some_and(|only| slot.track != only) {
                continue;
            }
            match slot.started {
                // Keep the earlier of a stop already queued and this one.
                Some(_) => slot.due = Some(slot.due.map_or(at, |due| due.min(at))),
                None => slot.due = None,
            }
        }
        self.slots.retain(|slot| slot.started.is_some());
    }

    /// Applies every change due at or before the playhead `at`, which the
    /// processor calls at a block part's start.
    pub(crate) fn apply_due(&mut self, at: Frames) {
        let mut i = 0;
        while i < self.slots.len() {
            let slot = &mut self.slots[i];
            let Some(due) = slot.due else {
                i += 1;
                continue;
            };
            if due > at {
                i += 1;
                continue;
            }
            slot.due = None;
            if slot.started.is_none() {
                // The queued launch plays from the frame it was queued for.
                slot.started = Some(due);
                i += 1;
            } else {
                // A queued stop: the clip is done.
                self.slots.swap_remove(i);
            }
        }
    }

    /// The first frame a change is due at, but after `at`: where the
    /// processor next splits its block.
    pub(crate) fn next_due(&self, at: Frames) -> Option<Frames> {
        self.slots
            .iter()
            .filter_map(|slot| slot.due)
            .filter(|&due| due > at)
            .min()
    }

    /// The slots' states, for the engine to publish.
    pub(crate) fn status(&self, out: &mut Vec<SlotStatus>) {
        out.clear();
        for slot in &self.slots {
            if out.len() < out.capacity() {
                out.push(SlotStatus {
                    track: slot.track,
                    scene: slot.scene,
                    state: slot.state(),
                });
            }
        }
    }

    /// What the tracks' players play: each slot whose clip sounds, with the
    /// frame it launched on.
    pub(crate) fn playback(&self, out: &mut Vec<SessionPlayback>) {
        out.clear();
        for slot in &self.slots {
            if let Some(frame) = slot.started
                && out.len() < out.capacity()
            {
                out.push(SessionPlayback {
                    track: slot.track,
                    scene: slot.scene,
                    frame,
                });
            }
        }
    }

    /// The slot for `(track, scene)`, if the launcher holds it.
    fn find_mut(&mut self, track: TrackId, scene: SceneId) -> Option<&mut Slot> {
        self.slots
            .iter_mut()
            .find(|slot| slot.track == track && slot.scene == scene)
    }

    #[cfg(test)]
    /// The slot's state, or `None` when it stopped.
    pub(crate) fn state_of(&self, track: TrackId, scene: SceneId) -> Option<SlotState> {
        self.slots
            .iter()
            .find(|slot| slot.track == track && slot.scene == scene)
            .map(|slot| slot.state())
    }
}

#[cfg(test)]
mod tests {
    use bl_core::time::Bpm;
    use bl_model::LaunchQuantization;

    use super::*;

    fn ids() -> (TrackId, SceneId) {
        let mut alloc = bl_core::id::IdAllocator::new();
        (TrackId(alloc.allocate()), SceneId(alloc.allocate()))
    }

    fn tempo() -> TempoMap {
        TempoMap::constant(Bpm::new(120.0).unwrap())
    }

    fn playhead<'a>(
        tempo: &'a TempoMap,
        signatures: &'a TimeSignatureMap,
        playing: bool,
        position: Ticks,
    ) -> Playhead<'a> {
        Playhead {
            wait_for_grid: playing,
            position,
            tempo,
            signatures,
            rate: SampleRate::HZ_48000,
        }
    }

    #[test]
    fn a_launch_while_playing_waits_for_the_grid_and_a_launch_while_stopped_doesnt() {
        // Spec: launch-quantisation R2, R3, R5
        let (track, scene) = ids();
        let tempo = tempo();
        let sigs = TimeSignatureMap::default();
        let mut launcher = SessionLauncher::new(16);
        // A quarter in: the 1-bar grid's next line is the bar line.
        let at = Ticks::QUARTER;
        launcher.launch(track, scene, playhead(&tempo, &sigs, true, at));
        assert_eq!(launcher.state_of(track, scene), Some(SlotState::Queued));
        let due = tempo.ticks_to_frames(Ticks::WHOLE, SampleRate::HZ_48000);
        assert_eq!(launcher.next_due(Frames::ZERO), Some(due));
        launcher.apply_due(due);
        assert_eq!(launcher.state_of(track, scene), Some(SlotState::Playing));
        let mut playing = Vec::with_capacity(4);
        launcher.playback(&mut playing);
        assert_eq!(
            playing,
            vec![SessionPlayback {
                track,
                scene,
                frame: due
            }]
        );
        // While stopped it plays at once.
        let (other, scene2) = ids();
        launcher.launch(other, scene2, playhead(&tempo, &sigs, false, at));
        assert_eq!(
            launcher.state_of(other, scene2),
            Some(SlotState::Queued),
            "the processor applies it at the playhead's frame in the same block"
        );
        launcher.apply_due(tempo.ticks_to_frames(at, SampleRate::HZ_48000));
        assert_eq!(launcher.state_of(other, scene2), Some(SlotState::Playing));
    }

    #[test]
    fn a_second_launch_on_a_track_takes_over_from_the_first() {
        // Spec: launch-quantisation R5
        let (track, scene) = ids();
        let (_, other) = ids();
        let tempo = tempo();
        let sigs = TimeSignatureMap::default();
        let mut launcher = SessionLauncher::new(16);
        launcher.launch(track, scene, playhead(&tempo, &sigs, true, Ticks::ZERO));
        launcher.launch(track, other, playhead(&tempo, &sigs, true, Ticks::QUARTER));
        assert_eq!(launcher.state_of(track, scene), None, "the queued one is dropped");
        assert_eq!(launcher.state_of(track, other), Some(SlotState::Queued));
        // And launching a third while the second plays queues the third and
        // makes the second stop where the third starts.
        let (_, third) = ids();
        let bar = tempo.ticks_to_frames(Ticks::WHOLE, SampleRate::HZ_48000);
        launcher.apply_due(bar);
        assert_eq!(launcher.state_of(track, other), Some(SlotState::Playing));
        launcher.launch(track, third, playhead(&tempo, &sigs, true, Ticks::WHOLE));
        assert_eq!(launcher.state_of(track, other), Some(SlotState::QueuedToStop));
        let next = tempo.ticks_to_frames(Ticks::WHOLE * 2, SampleRate::HZ_48000);
        launcher.apply_due(next);
        assert_eq!(launcher.state_of(track, other), None);
        assert_eq!(launcher.state_of(track, third), Some(SlotState::Playing));
    }

    #[test]
    fn a_stop_queues_for_the_grid_and_drops_a_queued_launch() {
        // Spec: launch-quantisation R6
        let (track, scene) = ids();
        let (_, other) = ids();
        let tempo = tempo();
        let sigs = TimeSignatureMap::default();
        let mut launcher = SessionLauncher::new(16);
        launcher.launch(track, scene, playhead(&tempo, &sigs, true, Ticks::ZERO));
        let bar = tempo.ticks_to_frames(Ticks::WHOLE, SampleRate::HZ_48000);
        launcher.apply_due(bar);
        assert_eq!(launcher.state_of(track, scene), Some(SlotState::Playing));
        // The next bar line, with the song a quarter in.
        let at = Ticks::WHOLE + Ticks::QUARTER;
        launcher.launch(track, other, playhead(&tempo, &sigs, true, at));
        launcher.stop(Some(track), playhead(&tempo, &sigs, true, at));
        assert_eq!(launcher.state_of(track, other), None, "the queued launch is dropped");
        assert_eq!(launcher.state_of(track, scene), Some(SlotState::QueuedToStop));
        let next = tempo.ticks_to_frames(Ticks::WHOLE * 2, SampleRate::HZ_48000);
        launcher.apply_due(next);
        assert_eq!(launcher.state_of(track, scene), None);
    }

    #[test]
    fn stopping_every_track_leaves_none_playing() {
        // Spec: launch-quantisation R6
        let tempo = tempo();
        let sigs = TimeSignatureMap::default();
        let mut launcher = SessionLauncher::new(16);
        let mut slots = Vec::new();
        for _ in 0..3 {
            let (track, scene) = ids();
            launcher.launch(track, scene, playhead(&tempo, &sigs, true, Ticks::ZERO));
            slots.push((track, scene));
        }
        let bar = tempo.ticks_to_frames(Ticks::WHOLE, SampleRate::HZ_48000);
        launcher.apply_due(bar);
        launcher.stop(None, playhead(&tempo, &sigs, true, Ticks::QUARTER));
        let next = tempo.ticks_to_frames(Ticks::WHOLE * 2, SampleRate::HZ_48000);
        launcher.apply_due(next);
        assert!(slots.iter().all(|&(t, s)| launcher.state_of(t, s).is_none()));
        let mut status = Vec::with_capacity(8);
        launcher.status(&mut status);
        assert!(status.is_empty());
    }

    #[test]
    fn a_scene_launch_queues_its_clips_on_one_frame() {
        // Spec: launch-quantisation R7
        let tempo = tempo();
        let sigs = TimeSignatureMap::default();
        let mut launcher = SessionLauncher::new(16);
        let mut alloc = bl_core::id::IdAllocator::new();
        let scene = SceneId(alloc.allocate());
        let slots: Vec<(SceneId, TrackId)> = (0..3)
            .map(|_| (scene, TrackId(alloc.allocate())))
            .collect();
        launcher.launch_scene(scene, &slots, playhead(&tempo, &sigs, true, Ticks::QUARTER));
        let mut playing = Vec::with_capacity(8);
        launcher.playback(&mut playing);
        assert!(playing.is_empty(), "queued, not playing yet");
        let bar = tempo.ticks_to_frames(Ticks::WHOLE, SampleRate::HZ_48000);
        launcher.apply_due(bar);
        launcher.playback(&mut playing);
        assert_eq!(playing.len(), 3);
        assert!(playing.iter().all(|entry| entry.frame == bar), "{playing:?}");
        assert!(playing.iter().all(|entry| entry.scene == scene));
    }

    #[test]
    fn quantisation_off_launches_at_once_and_a_queued_launch_keeps_its_frame() {
        // Spec: launch-quantisation R3, R4
        let (track, scene) = ids();
        let (_, other) = ids();
        let tempo = tempo();
        let sigs = TimeSignatureMap::default();
        let mut launcher = SessionLauncher::new(16);
        launcher.set_settings(LaunchSettings {
            quantization: LaunchQuantization::None,
            enabled: true,
        });
        launcher.launch(track, scene, playhead(&tempo, &sigs, true, Ticks::QUARTER));
        let now = tempo.ticks_to_frames(Ticks::QUARTER, SampleRate::HZ_48000);
        assert_eq!(launcher.next_due(Frames::ZERO), Some(now));
        launcher.apply_due(now);
        assert_eq!(launcher.state_of(track, scene), Some(SlotState::Playing));
        // A launch queued for the next bar keeps that frame when the value
        // changes (the maintainer's interim rule; see the spec's "To observe
        // in Live").
        launcher.set_settings(LaunchSettings::default());
        let at = Ticks::WHOLE + Ticks::QUARTER;
        launcher.launch(track, other, playhead(&tempo, &sigs, true, at));
        let due = tempo.ticks_to_frames(Ticks::WHOLE * 2, SampleRate::HZ_48000);
        assert_eq!(launcher.next_due(Frames::ZERO), Some(due));
        launcher.set_settings(LaunchSettings {
            quantization: LaunchQuantization::Sixteenth,
            enabled: true,
        });
        assert_eq!(
            launcher.next_due(Frames::ZERO),
            Some(due),
            "the queued launch keeps its frame"
        );
    }
}