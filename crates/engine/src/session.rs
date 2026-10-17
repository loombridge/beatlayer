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
//!
//! A Session clip that starts takes its track over from the Arrangement
//! (WP-3.6): the launcher keeps the tracks it holds, the players leave the
//! track's Arrangement clips silent, and only Back to Arrangement gives the
//! track back; a clip that stops doesn't.

use core::fmt;

use bl_commands::Context::{self, Global, Session};
use bl_commands::{CommandSpec, NoArgs, Permission, RegisterError, Registry};
use bl_core::time::{Frames, SampleRate, TempoMap, Ticks, TimeSignatureMap};
use bl_model::{LaunchSettings, SceneId, TrackId};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

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

/// A track a Session clip has taken over from the Arrangement, as its player
/// needs it: its Arrangement clips stay silent while it's listed, and the
/// slot clip playing on it, if any, sounds from its launch frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionPlayback {
    /// The track.
    pub track: TrackId,
    /// The scene whose slot clip plays, or `None` when the track is taken
    /// over and nothing plays on it.
    pub scene: Option<SceneId>,
    /// The song frame the clip launched on (zero when `scene` is `None`).
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
    /// Returns every track to the Arrangement at once (WP-3.6).
    BackToArrangement,
    /// Returns one track to the Arrangement at once (WP-3.6).
    TrackBackToArrangement(TrackId),
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
    /// The tracks a Session clip has taken over from the Arrangement, until
    /// a return (WP-3.6). Capacity as `slots`: a track is listed once.
    overridden: Vec<TrackId>,
    /// Stops that take a track over on a grid line without a Session clip
    /// playing on it: a stop button pressed on an idle track silences its
    /// Arrangement there.
    marks: Vec<(TrackId, Frames)>,
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
            overridden: Vec::with_capacity(capacity),
            marks: Vec::with_capacity(capacity),
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
        self.overridden.clear();
        self.marks.clear();
    }

    /// The frame a launch or a stop asked for now waits for: the next line
    /// of the quantisation grid, at once while the song is stopped or
    /// quantisation is off. The one place the grid is worked out.
    fn frame_for(&self, playhead: Playhead<'_>) -> Frames {
        if !playhead.wait_for_grid {
            return playhead.frames(playhead.position);
        }
        playhead.frames(
            self.settings
                .next_line(playhead.position, playhead.signatures),
        )
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
        // A slot that is already queued or playing is left alone: a second
        // launch of the same slot changes nothing, for now (the launch modes,
        // WP-3.3, decide what a second press means), and a queued slot keeps
        // the frame it was queued for (R4). One entry per slot.
        if self.find_mut(track, scene).is_some() {
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

    /// Stops the playing clip of `track`, or of every one of `tracks` with
    /// `None`, on the next grid line. A launch still queued on the track is
    /// dropped. A track no Session clip has taken over yet is taken over on
    /// that line, so its Arrangement goes silent there too (WP-3.6).
    pub(crate) fn stop(
        &mut self,
        track: Option<TrackId>,
        tracks: &[TrackId],
        playhead: Playhead<'_>,
    ) {
        let at = self.frame_for(playhead);
        match track {
            Some(track) => self.mark(track, at),
            None => tracks.iter().for_each(|&track| self.mark(track, at)),
        }
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

    /// Queues taking `track` over at `at`, keeping the earlier of two marks.
    fn mark(&mut self, track: TrackId, at: Frames) {
        if self.overridden.contains(&track) {
            return;
        }
        if let Some((_, due)) = self.marks.iter_mut().find(|(marked, _)| *marked == track) {
            *due = (*due).min(at);
        } else if self.marks.len() < self.marks.capacity() {
            self.marks.push((track, at));
        } else {
            self.overflowed += 1;
        }
    }

    /// Lists `track` as taken over from the Arrangement.
    fn take_over(&mut self, track: TrackId) {
        if self.overridden.contains(&track) {
            return;
        }
        if self.overridden.len() < self.overridden.capacity() {
            self.overridden.push(track);
        } else {
            self.overflowed += 1;
        }
    }

    /// Returns `track`, or every track with `None`, to the Arrangement at
    /// once: its slots stop whether they play or wait for the grid, a stop
    /// still queued is dropped, and the Arrangement plays from the playhead
    /// (WP-3.6).
    pub(crate) fn back_to_arrangement(&mut self, track: Option<TrackId>) {
        let returned = |t: TrackId| track.is_none_or(|only| only == t);
        self.slots.retain(|slot| !returned(slot.track));
        self.marks.retain(|&(t, _)| !returned(t));
        self.overridden.retain(|&t| !returned(t));
    }

    /// Forgets the takeovers of tracks the running program no longer has,
    /// so a deleted track doesn't keep Back to Arrangement lit.
    pub(crate) fn retain_tracks(&mut self, tracks: &[TrackId]) {
        self.overridden.retain(|t| tracks.contains(t));
        self.marks.retain(|(t, _)| tracks.contains(t));
    }

    /// The tracks a Session clip has taken over, for the engine to publish.
    pub(crate) fn overridden(&self) -> &[TrackId] {
        &self.overridden
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
                // The queued launch plays from the frame it was queued for,
                // and takes its track over from the Arrangement there.
                slot.started = Some(due);
                let track = slot.track;
                self.take_over(track);
                i += 1;
            } else {
                // A queued stop: the clip is done.
                self.slots.swap_remove(i);
            }
        }
        let mut i = 0;
        while i < self.marks.len() {
            if self.marks[i].1 <= at {
                let (track, _) = self.marks.swap_remove(i);
                self.take_over(track);
            } else {
                i += 1;
            }
        }
    }

    /// The first frame a change is due at, but after `at`: where the
    /// processor next splits its block.
    pub(crate) fn next_due(&self, at: Frames) -> Option<Frames> {
        self.slots
            .iter()
            .filter_map(|slot| slot.due)
            .chain(self.marks.iter().map(|&(_, due)| due))
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
    /// frame it launched on, and each taken-over track with nothing playing.
    pub(crate) fn playback(&self, out: &mut Vec<SessionPlayback>) {
        out.clear();
        for slot in &self.slots {
            if let Some(frame) = slot.started
                && out.len() < out.capacity()
            {
                out.push(SessionPlayback {
                    track: slot.track,
                    scene: Some(slot.scene),
                    frame,
                });
            }
        }
        for &track in &self.overridden {
            if out.len() < out.capacity() && !out.iter().any(|entry| entry.track == track) {
                out.push(SessionPlayback {
                    track,
                    scene: None,
                    frame: Frames::ZERO,
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

/// The arguments of `session.launch`: the slot whose clip launches.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LaunchArgs {
    /// The track whose slot it is.
    pub track: TrackId,
    /// The scene whose slot it is.
    pub scene: SceneId,
}

/// The arguments of `session.launch_scene`: the scene to launch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LaunchSceneArgs {
    /// The scene.
    pub scene: SceneId,
}

/// The arguments of `session.stop_track`: the track whose clip stops.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StopTrackArgs {
    /// The track.
    pub track: TrackId,
}

/// The arguments of `session.back_to_arrangement_track`: the track that
/// returns to the Arrangement.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BackToArrangementTrackArgs {
    /// The track.
    pub track: TrackId,
}

/// Adds the Session View's launch commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    for entry in ENTRIES {
        registry.register(entry.spec())?;
    }
    Ok(())
}

/// Whether `id` is one of the commands [`register`] adds, so a host knows
/// to send it to [`command_from_args`].
#[must_use]
pub fn handles(id: &str) -> bool {
    ENTRIES.iter().any(|entry| entry.id == id)
}

/// The [`SessionCommand`] a launch command's ID and JSON arguments
/// describe, for the host to send to the engine.
///
/// # Errors
///
/// [`RunError::UnknownCommand`] if `id` isn't a launch command, and
/// [`RunError::InvalidArgs`] if the arguments don't fit its schema.
pub fn command_from_args(id: &str, args: Value) -> Result<SessionCommand, RunError> {
    let entry = ENTRIES
        .iter()
        .find(|entry| entry.id == id)
        .ok_or_else(|| RunError::UnknownCommand(id.to_owned()))?;
    let invalid = |error: serde_json::Error| RunError::InvalidArgs {
        command: entry.id,
        message: error.to_string(),
    };
    match entry.action {
        Action::Launch => {
            let LaunchArgs { track, scene } = serde_json::from_value(args).map_err(invalid)?;
            Ok(SessionCommand::Launch { track, scene })
        }
        Action::LaunchScene => {
            let LaunchSceneArgs { scene } = serde_json::from_value(args).map_err(invalid)?;
            Ok(SessionCommand::LaunchScene(scene))
        }
        Action::StopTrack => {
            let StopTrackArgs { track } = serde_json::from_value(args).map_err(invalid)?;
            Ok(SessionCommand::StopTrack(track))
        }
        Action::StopAll => {
            serde_json::from_value::<NoArgs>(args).map_err(invalid)?;
            Ok(SessionCommand::StopAll)
        }
        Action::BackToArrangement => {
            serde_json::from_value::<NoArgs>(args).map_err(invalid)?;
            Ok(SessionCommand::BackToArrangement)
        }
        Action::TrackBackToArrangement => {
            let BackToArrangementTrackArgs { track } =
                serde_json::from_value(args).map_err(invalid)?;
            Ok(SessionCommand::TrackBackToArrangement(track))
        }
    }
}

/// Why [`command_from_args`] refused a command.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RunError {
    /// No Session View launch command has this ID.
    UnknownCommand(String),
    /// The arguments don't fit the command's schema.
    InvalidArgs {
        /// The command.
        command: &'static str,
        /// What's wrong, from the deserialiser.
        message: String,
    },
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownCommand(id) => write!(f, "no Session View command is called `{id}`"),
            Self::InvalidArgs { command, message } => {
                write!(f, "invalid arguments for `{command}`: {message}")
            }
        }
    }
}

impl std::error::Error for RunError {}

/// What running a registered command sends.
#[derive(Clone, Copy)]
enum Action {
    /// [`SessionCommand::Launch`].
    Launch,
    /// [`SessionCommand::LaunchScene`].
    LaunchScene,
    /// [`SessionCommand::StopTrack`].
    StopTrack,
    /// [`SessionCommand::StopAll`].
    StopAll,
    /// [`SessionCommand::BackToArrangement`].
    BackToArrangement,
    /// [`SessionCommand::TrackBackToArrangement`].
    TrackBackToArrangement,
}

/// One registered command.
struct Entry {
    id: &'static str,
    action: Action,
    label: &'static str,
    contexts: &'static [Context],
    /// The shortcut that runs it, if one does.
    key: Option<&'static str>,
    description: &'static str,
}

impl Entry {
    fn spec(&self) -> CommandSpec {
        // Launching and stopping change nothing in the set, so they're
        // `read` (D17's permission classes), as the transport's playback
        // commands are.
        let mut spec = CommandSpec::new(self.id, self.label)
            .description(self.description)
            .permission(Permission::Read)
            .contexts(self.contexts.iter().copied())
            .result::<()>();
        if let Some(key) = self.key {
            spec = spec.binding(key);
        }
        spec = match self.action {
            Action::Launch => spec.args::<LaunchArgs>(),
            Action::LaunchScene => spec.args::<LaunchSceneArgs>(),
            Action::StopTrack => spec.args::<StopTrackArgs>(),
            Action::StopAll | Action::BackToArrangement => spec.args::<NoArgs>(),
            Action::TrackBackToArrangement => spec.args::<BackToArrangementTrackArgs>(),
        };
        spec
    }
}

/// Every Session View launch command this crate registers. The rules behind
/// them are in the launch quantisation spec.
const ENTRIES: &[Entry] = &[
    Entry {
        id: "session.launch",
        action: Action::Launch,
        label: "Launch Clip",
        contexts: &[Session],
        key: None,
        description: "Launches the clip in a track's Session slot at `scene`, as clicking the \
            slot's launch button does. While the song plays the clip starts on the next line of \
            the global launch quantisation grid, and until then the slot reads `queued`; while \
            it's stopped the transport starts at once from the Arrangement position and the \
            clip starts with it. A clip already playing on that track stops where the new one \
            starts. The engine publishes the slots' states with the playhead, so the UI can \
            draw them. It changes nothing in the set.",
    },
    Entry {
        id: "session.launch_scene",
        action: Action::LaunchScene,
        label: "Launch Scene",
        contexts: &[Session],
        key: None,
        description: "Launches every clip of the scene at `scene`, as clicking the scene's \
            launch button does: all of them start on one line of the launch quantisation grid, \
            as one launch, so a scene's clips play together. While the song is stopped the \
            transport starts at once. It changes nothing in the set.",
    },
    Entry {
        id: "session.stop_track",
        action: Action::StopTrack,
        label: "Stop Track",
        contexts: &[Session],
        key: None,
        description: "Stops the clip the track plays, as the track's stop button in the status \
            row does: it stops on the next line of the launch quantisation grid, and until then \
            the slot reads `queued to stop`. A launch still queued on the track is dropped, and \
            the transport keeps playing. It changes nothing in the set.",
    },
    Entry {
        id: "session.stop_all",
        action: Action::StopAll,
        label: "Stop All Clips",
        contexts: &[Session],
        key: None,
        description: "Stops every track's playing clip, as Stop All Clips at the Main column's \
            left does: all of them stop on the next line of the launch quantisation grid, as \
            one stop. Launches still queued are dropped, and the transport keeps playing. It \
            changes nothing in the set.",
    },
    Entry {
        id: "session.back_to_arrangement",
        action: Action::BackToArrangement,
        label: "Back to Arrangement",
        contexts: &[Global],
        key: Some("S-TRANS-009"),
        description: "Returns every track to the Arrangement, as the Back to Arrangement button \
            and F10 do. A Session clip that started has taken its track over from the \
            Arrangement, and stays so after it stops, until this runs. It stops every track's \
            Session clips at once, not on the launch quantisation grid, and drops launches and \
            stops still queued; the Arrangement plays again from the playhead and the \
            transport keeps running. It changes nothing in the set.",
    },
    Entry {
        id: "session.back_to_arrangement_track",
        action: Action::TrackBackToArrangement,
        label: "Back to Arrangement (Track)",
        contexts: &[Global],
        key: None,
        description: "Returns one track to the Arrangement, as the button in the track's \
            Arrangement header does: its Session clip stops at once, a launch or a stop still \
            queued on it is dropped, and its Arrangement clips play again from the playhead. \
            Other tracks are not touched, and the transport keeps running. A track nothing \
            took over just keeps playing its Arrangement. It changes nothing in the set.",
    },
];

#[cfg(test)]
mod tests {
    use bl_core::time::Bpm;
    use bl_model::LaunchQuantization;

    use super::*;

    fn ids() -> (TrackId, SceneId) {
        use std::sync::atomic::{AtomicU64, Ordering};
        // `IdAllocator::new` hands out the same IDs to every caller, so a
        // counter shared by the binary's tests is what keeps two calls apart.
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let next = NEXT.fetch_add(2, Ordering::Relaxed);
        (
            TrackId(bl_core::id::Id::new(next).unwrap()),
            SceneId(bl_core::id::Id::new(next + 1).unwrap()),
        )
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
        // Spec: launch-quantisation R12. A second launch of the slot that
        // plays changes nothing: what a second press does is the launch
        // mode's business (WP-3.3).
        launcher.launch(
            track,
            scene,
            playhead(&tempo, &sigs, true, Ticks::WHOLE * 3),
        );
        assert_eq!(launcher.state_of(track, scene), Some(SlotState::Playing));
        assert_eq!(launcher.next_due(Frames::ZERO), None);
        let mut playing = Vec::with_capacity(4);
        launcher.playback(&mut playing);
        assert_eq!(
            playing,
            vec![SessionPlayback {
                track,
                scene: Some(scene),
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
        assert_eq!(
            launcher.state_of(track, scene),
            None,
            "the queued one is dropped"
        );
        assert_eq!(launcher.state_of(track, other), Some(SlotState::Queued));
        // And launching a third while the second plays queues the third and
        // makes the second stop where the third starts.
        let (_, third) = ids();
        let bar = tempo.ticks_to_frames(Ticks::WHOLE, SampleRate::HZ_48000);
        launcher.apply_due(bar);
        assert_eq!(launcher.state_of(track, other), Some(SlotState::Playing));
        launcher.launch(track, third, playhead(&tempo, &sigs, true, Ticks::WHOLE));
        assert_eq!(
            launcher.state_of(track, other),
            Some(SlotState::QueuedToStop)
        );
        let next = tempo.ticks_to_frames(Ticks::WHOLE * 2, SampleRate::HZ_48000);
        launcher.apply_due(next);
        assert_eq!(launcher.state_of(track, other), None);
        assert_eq!(launcher.state_of(track, third), Some(SlotState::Playing));
    }

    #[test]
    fn a_second_launch_of_the_same_slot_leaves_one_entry() {
        // Spec: launch-quantisation R12. A second launch of a slot that is
        // queued or playing changes nothing, so the launcher holds one entry
        // per slot.
        let (track, scene) = ids();
        let tempo = tempo();
        let sigs = TimeSignatureMap::default();
        let mut launcher = SessionLauncher::new(16);
        launcher.launch(track, scene, playhead(&tempo, &sigs, true, Ticks::QUARTER));
        let due = tempo.ticks_to_frames(Ticks::WHOLE, SampleRate::HZ_48000);
        launcher.launch(track, scene, playhead(&tempo, &sigs, true, Ticks::QUARTER));
        let mut status = Vec::with_capacity(8);
        launcher.status(&mut status);
        assert_eq!(status.len(), 1, "{status:?}");
        assert_eq!(
            launcher.next_due(Frames::ZERO),
            Some(due),
            "the frame it was queued for stays"
        );
        launcher.apply_due(due);
        launcher.launch(
            track,
            scene,
            playhead(&tempo, &sigs, true, Ticks::WHOLE * 3),
        );
        launcher.status(&mut status);
        assert_eq!(status.len(), 1, "{status:?}");
        assert_eq!(launcher.state_of(track, scene), Some(SlotState::Playing));
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
        launcher.stop(Some(track), &[], playhead(&tempo, &sigs, true, at));
        assert_eq!(
            launcher.state_of(track, other),
            None,
            "the queued launch is dropped"
        );
        assert_eq!(
            launcher.state_of(track, scene),
            Some(SlotState::QueuedToStop)
        );
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
        launcher.stop(None, &[], playhead(&tempo, &sigs, true, Ticks::QUARTER));
        let next = tempo.ticks_to_frames(Ticks::WHOLE * 2, SampleRate::HZ_48000);
        launcher.apply_due(next);
        assert!(
            slots
                .iter()
                .all(|&(t, s)| launcher.state_of(t, s).is_none())
        );
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
        let slots: Vec<(SceneId, TrackId)> =
            (0..3).map(|_| (scene, TrackId(alloc.allocate()))).collect();
        launcher.launch_scene(scene, &slots, playhead(&tempo, &sigs, true, Ticks::QUARTER));
        let mut playing = Vec::with_capacity(8);
        launcher.playback(&mut playing);
        assert!(playing.is_empty(), "queued, not playing yet");
        let bar = tempo.ticks_to_frames(Ticks::WHOLE, SampleRate::HZ_48000);
        launcher.apply_due(bar);
        launcher.playback(&mut playing);
        assert_eq!(playing.len(), 3);
        assert!(
            playing.iter().all(|entry| entry.frame == bar),
            "{playing:?}"
        );
        assert!(playing.iter().all(|entry| entry.scene == Some(scene)));
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

    #[test]
    fn a_started_clip_takes_its_track_over_and_a_stop_leaves_it_so() {
        // Spec: session-view R20
        let (track, scene) = ids();
        let tempo = tempo();
        let sigs = TimeSignatureMap::default();
        let mut launcher = SessionLauncher::new(16);
        let mut playing = Vec::with_capacity(4);
        launcher.launch(track, scene, playhead(&tempo, &sigs, true, Ticks::QUARTER));
        launcher.playback(&mut playing);
        assert!(
            playing.is_empty() && launcher.overridden().is_empty(),
            "a queued launch leaves the Arrangement sounding until its line"
        );
        let bar = tempo.ticks_to_frames(Ticks::WHOLE, SampleRate::HZ_48000);
        launcher.apply_due(bar);
        assert_eq!(launcher.overridden(), [track]);
        launcher.stop(
            Some(track),
            &[],
            playhead(&tempo, &sigs, true, Ticks::WHOLE),
        );
        launcher.apply_due(tempo.ticks_to_frames(Ticks::WHOLE * 2, SampleRate::HZ_48000));
        assert_eq!(launcher.state_of(track, scene), None);
        assert_eq!(launcher.overridden(), [track], "the track stays taken over");
        launcher.playback(&mut playing);
        assert_eq!(
            playing,
            vec![SessionPlayback {
                track,
                scene: None,
                frame: Frames::ZERO
            }]
        );
    }

    #[test]
    fn a_stop_on_an_idle_track_takes_it_over_on_the_grid() {
        // Spec: session-view R21
        let (track, _) = ids();
        let (idle, _) = ids();
        let tempo = tempo();
        let sigs = TimeSignatureMap::default();
        let mut launcher = SessionLauncher::new(16);
        launcher.stop(
            None,
            &[track, idle],
            playhead(&tempo, &sigs, true, Ticks::QUARTER),
        );
        let bar = tempo.ticks_to_frames(Ticks::WHOLE, SampleRate::HZ_48000);
        assert_eq!(launcher.next_due(Frames::ZERO), Some(bar));
        assert!(launcher.overridden().is_empty());
        launcher.apply_due(bar);
        assert_eq!(launcher.overridden(), [track, idle]);
        assert_eq!(launcher.next_due(bar), None);
    }

    #[test]
    fn a_return_stops_at_once_and_drops_what_is_queued() {
        // Spec: session-view R22, R23
        let (a, scene_a) = ids();
        let (b, scene_b) = ids();
        let (c, scene_c) = ids();
        let tempo = tempo();
        let sigs = TimeSignatureMap::default();
        let mut launcher = SessionLauncher::new(16);
        let at = playhead(&tempo, &sigs, true, Ticks::ZERO);
        launcher.launch(a, scene_a, at);
        launcher.launch(b, scene_b, at);
        let bar = tempo.ticks_to_frames(Ticks::WHOLE, SampleRate::HZ_48000);
        launcher.apply_due(bar);
        // `a` plays with a stop queued, `b` plays, `c` has a launch queued
        // and an idle stop's mark.
        let later = playhead(&tempo, &sigs, true, Ticks::WHOLE + Ticks::QUARTER);
        launcher.stop(Some(a), &[], later);
        launcher.launch(c, scene_c, later);
        launcher.stop(Some(c), &[], later);
        launcher.launch(c, scene_c, later);
        launcher.back_to_arrangement(Some(a));
        assert_eq!(launcher.state_of(a, scene_a), None);
        assert_eq!(launcher.overridden(), [b], "only that track returned");
        assert_eq!(launcher.state_of(b, scene_b), Some(SlotState::Playing));
        launcher.back_to_arrangement(None);
        let mut status = Vec::with_capacity(8);
        launcher.status(&mut status);
        assert!(status.is_empty() && launcher.overridden().is_empty());
        assert_eq!(launcher.next_due(Frames::ZERO), None, "nothing is queued");
    }

    #[test]
    fn a_deleted_track_is_forgotten() {
        // Spec: session-view R24
        let (kept, _) = ids();
        let (gone, _) = ids();
        let tempo = tempo();
        let sigs = TimeSignatureMap::default();
        let mut launcher = SessionLauncher::new(16);
        let at = playhead(&tempo, &sigs, false, Ticks::ZERO);
        launcher.stop(None, &[kept, gone], at);
        launcher.apply_due(Frames::ZERO);
        assert_eq!(launcher.overridden(), [kept, gone]);
        launcher.retain_tracks(&[kept]);
        assert_eq!(launcher.overridden(), [kept]);
    }

    #[test]
    fn every_entry_registers_complete() {
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        assert_eq!(registry.len(), ENTRIES.len());
        assert_eq!(registry.problems(), Vec::new());
        assert!(ENTRIES.iter().all(|entry| handles(entry.id)));
        assert!(!handles("session.set_quantization"), "a shell command");
    }

    #[test]
    fn arguments_are_checked() {
        // Spec: launch-quantisation R9
        let (track, scene) = ids();
        assert_eq!(
            command_from_args(
                "session.launch",
                serde_json::json!({ "track": track, "scene": scene })
            ),
            Ok(SessionCommand::Launch { track, scene })
        );
        assert_eq!(
            command_from_args(
                "session.launch_scene",
                serde_json::json!({ "scene": scene })
            ),
            Ok(SessionCommand::LaunchScene(scene))
        );
        assert_eq!(
            command_from_args("session.stop_track", serde_json::json!({ "track": track })),
            Ok(SessionCommand::StopTrack(track))
        );
        assert_eq!(
            command_from_args("session.stop_all", serde_json::json!({})),
            Ok(SessionCommand::StopAll)
        );
        assert!(matches!(
            command_from_args("session.launch", serde_json::json!({})),
            Err(RunError::InvalidArgs { .. })
        ));
        assert_eq!(
            command_from_args("session.stop", serde_json::json!({})),
            Err(RunError::UnknownCommand("session.stop".to_owned()))
        );
    }
}
