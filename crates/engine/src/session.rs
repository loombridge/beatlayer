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

use core::fmt;

use bl_commands::Context::{self, Session};
use bl_commands::{CommandSpec, NoArgs, Permission, RegisterError, Registry};
use bl_core::time::{Frames, SampleRate, TempoMap, Ticks, TimeSignatureMap};
use bl_model::{LaunchMode, LaunchQuantization, LaunchSettings, SceneId, TrackId};
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
    /// Releases the launch button of a slot: a Gate or Repeat clip stops on
    /// the grid, and the other modes ignore it (WP-3.3).
    Release {
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
    /// The frame it changes at: where a queued launch plays from, where a
    /// queued restart plays again from, or where a queued stop lands.
    due: Option<Frames>,
    /// Whether the change at `due` of a slot that plays is a restart rather
    /// than a stop (the launch quantisation spec, R14).
    restart: bool,
    /// A stop to queue once the launch or restart at `due` has happened:
    /// a Gate or Repeat button was released before the clip started (R16).
    then_stop: Option<Frames>,
    /// The grid tick the last launch or restart was queued for, which the
    /// stop after it and Repeat's next line are counted from.
    line: Ticks,
    /// Whether a Repeat button is held, so the clip restarts on each line
    /// (R18).
    repeating: bool,
    /// Whether the queued restart is one Repeat queued itself, which a
    /// release turns into the stop (R18).
    auto: bool,
}

impl Slot {
    /// Makes a clip that plays stop at `at`, dropping the restart, the stop
    /// behind it and the Repeat that were waiting.
    fn stop_at(&mut self, at: Frames) {
        self.due = Some(at);
        self.restart = false;
        self.then_stop = None;
        self.repeating = false;
        self.auto = false;
    }

    /// What the slot is doing, for the UI.
    fn state(self) -> SlotState {
        match (self.started, self.due) {
            (Some(_), Some(_)) if !self.restart => SlotState::QueuedToStop,
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

    /// The tick of the grid line a launch or a stop asked for now waits for:
    /// the next line of the quantisation grid, at once while the song is
    /// stopped or quantisation is off. The one place the grid is worked out.
    fn line_for(&self, playhead: Playhead<'_>) -> Ticks {
        if !playhead.wait_for_grid {
            return playhead.position;
        }
        self.settings
            .next_line(playhead.position, playhead.signatures)
    }

    /// The frame a launch or a stop asked for now waits for.
    fn frame_for(&self, playhead: Playhead<'_>) -> Frames {
        playhead.frames(self.line_for(playhead))
    }

    /// Whether Repeat has a step to repeat at: with quantisation off it acts
    /// as Trigger (R18).
    fn steps(&self) -> bool {
        self.settings.enabled && self.settings.quantization != LaunchQuantization::None
    }

    /// A press of the launch button of `(track, scene)`, whose clip has
    /// launch mode `mode`: what it does is the mode's rule (R12 to R18).
    pub(crate) fn press(
        &mut self,
        track: TrackId,
        scene: SceneId,
        mode: LaunchMode,
        playhead: Playhead<'_>,
    ) {
        let line = self.line_for(playhead);
        let at = playhead.frames(line);
        self.press_at(track, scene, mode, line, at);
    }

    /// A release of the launch button of `(track, scene)`, whose clip has
    /// launch mode `mode`. Only Gate and Repeat act on it (R15, R18).
    pub(crate) fn release(
        &mut self,
        track: TrackId,
        scene: SceneId,
        mode: LaunchMode,
        playhead: Playhead<'_>,
    ) {
        if !matches!(mode, LaunchMode::Gate | LaunchMode::Repeat) {
            return;
        }
        let settings = self.settings;
        let at = self.frame_for(playhead);
        let Some(slot) = self.find_mut(track, scene) else {
            return;
        };
        slot.repeating = false;
        if slot.auto && slot.restart && slot.due.is_some() {
            // The clip plays out the quantum it is in (R18).
            slot.stop_at(at);
        } else if slot.started.is_none() || slot.restart {
            // The release never cancels a queued launch: the stop lands on
            // the line after it, so the clip plays one quantum (R16).
            let after = settings.next_line(slot.line, playhead.signatures);
            slot.then_stop = Some(playhead.frames(after));
        } else {
            // Keep the earlier of a stop already queued and this one.
            slot.stop_at(slot.due.map_or(at, |due| due.min(at)));
        }
    }

    /// Launches every slot of `scene` that `slots` names (`(scene, track,
    /// mode)` triples, from the running program), all on one frame: a scene's
    /// clips start together (observed in Live 12.4.3). A scene launch acts
    /// as a Trigger press on each, whatever the clip's mode.
    pub(crate) fn launch_scene(
        &mut self,
        scene: SceneId,
        slots: &[(SceneId, TrackId, LaunchMode)],
        playhead: Playhead<'_>,
    ) {
        let line = self.line_for(playhead);
        let at = playhead.frames(line);
        for &(_, track, _) in slots {
            self.press_at(track, scene, LaunchMode::Trigger, line, at);
        }
    }

    /// A press of `(track, scene)`'s launch button with its grid line already
    /// worked out, so a scene's slots share one frame.
    fn press_at(
        &mut self,
        track: TrackId,
        scene: SceneId,
        mode: LaunchMode,
        line: Ticks,
        at: Frames,
    ) {
        let repeating = mode == LaunchMode::Repeat && self.steps();
        let Some(slot) = self.find_mut(track, scene) else {
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
                restart: false,
                then_stop: None,
                line,
                repeating,
                auto: false,
            });
            return;
        };
        slot.then_stop = None;
        slot.auto = false;
        slot.repeating = repeating;
        if slot.started.is_none() {
            // Still queued to start: it keeps the frame it was queued for
            // (R4), and a second press changes nothing.
            return;
        }
        let queued_stop = slot.due.is_some() && !slot.restart;
        if mode == LaunchMode::Toggle && !queued_stop {
            // Toggle: the press after a start queues the stop (R17).
            slot.stop_at(at);
        } else if !(slot.due.is_some() && slot.restart) {
            // Trigger, Gate and Repeat restart the clip on the grid; so does
            // a Toggle press while its stop is queued (R14, R17).
            slot.due = Some(at);
            slot.restart = true;
            slot.line = line;
            self.clear_way_for(track, scene, at);
        }
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
                Some(_) => slot.stop_at(at),
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
                Some(_) => slot.stop_at(match (slot.due, slot.restart) {
                    (Some(due), false) => due.min(at),
                    _ => at,
                }),
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
            if slot.started.is_none() || slot.restart {
                // The queued launch or restart plays from the frame it was
                // queued for; a stop queued behind it follows.
                slot.started = Some(due);
                slot.restart = false;
                slot.auto = false;
                if let Some(stop) = slot.then_stop.take() {
                    slot.due = Some(stop);
                    continue;
                }
                i += 1;
            } else {
                // A queued stop: the clip is done.
                self.slots.swap_remove(i);
            }
        }
    }

    /// Queues the next restart of every slot whose Repeat button is held and
    /// whose clip plays, one quantisation step after the last (R18). The
    /// processor calls it after [`Self::apply_due`].
    pub(crate) fn repeat(&mut self, playhead: Playhead<'_>) {
        if !self.steps() {
            return;
        }
        let settings = self.settings;
        for slot in &mut self.slots {
            if slot.repeating && slot.started.is_some() && slot.due.is_none() {
                slot.line = settings.next_line(slot.line, playhead.signatures);
                slot.due = Some(playhead.frames(slot.line));
                slot.restart = true;
                slot.auto = true;
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
        Action::Release => {
            let LaunchArgs { track, scene } = serde_json::from_value(args).map_err(invalid)?;
            Ok(SessionCommand::Release { track, scene })
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
    /// [`SessionCommand::Release`].
    Release,
    /// [`SessionCommand::LaunchScene`].
    LaunchScene,
    /// [`SessionCommand::StopTrack`].
    StopTrack,
    /// [`SessionCommand::StopAll`].
    StopAll,
}

/// One registered command.
struct Entry {
    id: &'static str,
    action: Action,
    label: &'static str,
    contexts: &'static [Context],
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
        spec = match self.action {
            Action::Launch | Action::Release => spec.args::<LaunchArgs>(),
            Action::LaunchScene => spec.args::<LaunchSceneArgs>(),
            Action::StopTrack => spec.args::<StopTrackArgs>(),
            Action::StopAll => spec.args::<NoArgs>(),
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
        description: "Launches the clip in a track's Session slot at `scene`, as clicking the \
            slot's launch button does. While the song plays the clip starts on the next line of \
            the global launch quantisation grid, and until then the slot reads `queued`; while \
            it's stopped the transport starts at once from the Arrangement position and the \
            clip starts with it. A clip already playing on that track stops where the new one \
            starts. The clip's launch mode (`clip.set_launch_mode`) says what the press does \
            next: a Trigger, Gate or Repeat clip that already plays restarts on the grid, and a \
            Toggle clip that plays is queued to stop. A Gate or Repeat clip stops when \
            `session.release` follows. The engine publishes the slots' states with the \
            playhead, so the UI can draw them. It changes nothing in the set.",
    },
    Entry {
        id: "session.release",
        action: Action::Release,
        label: "Release Clip Launch Button",
        contexts: &[Session],
        description: "Ends the press that `session.launch` began on the slot at `track` and \
            `scene`, as letting go of the slot's launch button does. A clip in Gate mode stops \
            on the next line of its launch quantisation grid, or at once with quantisation off, \
            and a Repeat clip stops the same way after the quantum it is playing; a release \
            never cancels a launch still queued, so the clip plays one quantum before it \
            stops. A Trigger or Toggle clip ignores the release, and so does a slot that \
            isn't playing. The clip's mode is read at the release. It changes nothing in the \
            set.",
    },
    Entry {
        id: "session.launch_scene",
        action: Action::LaunchScene,
        label: "Launch Scene",
        contexts: &[Session],
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
        description: "Stops every track's playing clip, as Stop All Clips at the Main column's \
            left does: all of them stop on the next line of the launch quantisation grid, as \
            one stop. Launches still queued are dropped, and the transport keeps playing. It \
            changes nothing in the set.",
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
        launcher.press(
            track,
            scene,
            LaunchMode::Trigger,
            playhead(&tempo, &sigs, true, at),
        );
        assert_eq!(launcher.state_of(track, scene), Some(SlotState::Queued));
        let due = tempo.ticks_to_frames(Ticks::WHOLE, SampleRate::HZ_48000);
        assert_eq!(launcher.next_due(Frames::ZERO), Some(due));
        launcher.apply_due(due);
        assert_eq!(launcher.state_of(track, scene), Some(SlotState::Playing));
        assert_eq!(launcher.next_due(Frames::ZERO), None);
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
        launcher.press(
            other,
            scene2,
            LaunchMode::Trigger,
            playhead(&tempo, &sigs, false, at),
        );
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
        launcher.press(
            track,
            scene,
            LaunchMode::Trigger,
            playhead(&tempo, &sigs, true, Ticks::ZERO),
        );
        launcher.press(
            track,
            other,
            LaunchMode::Trigger,
            playhead(&tempo, &sigs, true, Ticks::QUARTER),
        );
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
        launcher.press(
            track,
            third,
            LaunchMode::Trigger,
            playhead(&tempo, &sigs, true, Ticks::WHOLE),
        );
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
        // queued changes nothing, so the launcher holds one entry per slot.
        let (track, scene) = ids();
        let tempo = tempo();
        let sigs = TimeSignatureMap::default();
        let mut launcher = SessionLauncher::new(16);
        launcher.press(
            track,
            scene,
            LaunchMode::Trigger,
            playhead(&tempo, &sigs, true, Ticks::QUARTER),
        );
        let due = tempo.ticks_to_frames(Ticks::WHOLE, SampleRate::HZ_48000);
        launcher.press(
            track,
            scene,
            LaunchMode::Trigger,
            playhead(&tempo, &sigs, true, Ticks::QUARTER),
        );
        let mut status = Vec::with_capacity(8);
        launcher.status(&mut status);
        assert_eq!(status.len(), 1, "{status:?}");
        assert_eq!(
            launcher.next_due(Frames::ZERO),
            Some(due),
            "the frame it was queued for stays"
        );
        launcher.apply_due(due);
        // Spec: launch-quantisation R14. A press of a playing Trigger clip
        // queues a restart of the same slot, so there is still one entry.
        launcher.press(
            track,
            scene,
            LaunchMode::Trigger,
            playhead(&tempo, &sigs, true, Ticks::WHOLE * 3),
        );
        launcher.status(&mut status);
        assert_eq!(status.len(), 1, "{status:?}");
        assert_eq!(launcher.state_of(track, scene), Some(SlotState::Queued));
    }

    #[test]
    fn a_stop_queues_for_the_grid_and_drops_a_queued_launch() {
        // Spec: launch-quantisation R6
        let (track, scene) = ids();
        let (_, other) = ids();
        let tempo = tempo();
        let sigs = TimeSignatureMap::default();
        let mut launcher = SessionLauncher::new(16);
        launcher.press(
            track,
            scene,
            LaunchMode::Trigger,
            playhead(&tempo, &sigs, true, Ticks::ZERO),
        );
        let bar = tempo.ticks_to_frames(Ticks::WHOLE, SampleRate::HZ_48000);
        launcher.apply_due(bar);
        assert_eq!(launcher.state_of(track, scene), Some(SlotState::Playing));
        // The next bar line, with the song a quarter in.
        let at = Ticks::WHOLE + Ticks::QUARTER;
        launcher.press(
            track,
            other,
            LaunchMode::Trigger,
            playhead(&tempo, &sigs, true, at),
        );
        launcher.stop(Some(track), playhead(&tempo, &sigs, true, at));
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
            launcher.press(
                track,
                scene,
                LaunchMode::Trigger,
                playhead(&tempo, &sigs, true, Ticks::ZERO),
            );
            slots.push((track, scene));
        }
        let bar = tempo.ticks_to_frames(Ticks::WHOLE, SampleRate::HZ_48000);
        launcher.apply_due(bar);
        launcher.stop(None, playhead(&tempo, &sigs, true, Ticks::QUARTER));
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
        let slots: Vec<(SceneId, TrackId, LaunchMode)> = (0..3)
            .map(|_| (scene, TrackId(alloc.allocate()), LaunchMode::Trigger))
            .collect();
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
        launcher.press(
            track,
            scene,
            LaunchMode::Trigger,
            playhead(&tempo, &sigs, true, Ticks::QUARTER),
        );
        let now = tempo.ticks_to_frames(Ticks::QUARTER, SampleRate::HZ_48000);
        assert_eq!(launcher.next_due(Frames::ZERO), Some(now));
        launcher.apply_due(now);
        assert_eq!(launcher.state_of(track, scene), Some(SlotState::Playing));
        // A launch queued for the next bar keeps that frame when the value
        // changes (the maintainer's interim rule; see the spec's "To observe
        // in Live").
        launcher.set_settings(LaunchSettings::default());
        let at = Ticks::WHOLE + Ticks::QUARTER;
        launcher.press(
            track,
            other,
            LaunchMode::Trigger,
            playhead(&tempo, &sigs, true, at),
        );
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

    /// The song frame of `ticks` at the tests' tempo.
    fn frame(ticks: Ticks) -> Frames {
        tempo().ticks_to_frames(ticks, SampleRate::HZ_48000)
    }

    /// A launcher whose `mode` clip plays from the first bar line.
    fn playing(mode: LaunchMode) -> (SessionLauncher, TrackId, SceneId) {
        let (track, scene) = ids();
        let mut launcher = SessionLauncher::new(16);
        let (tempo, sigs) = (tempo(), TimeSignatureMap::default());
        launcher.press(
            track,
            scene,
            mode,
            playhead(&tempo, &sigs, true, Ticks::ZERO),
        );
        launcher.apply_due(frame(Ticks::WHOLE));
        assert_eq!(launcher.state_of(track, scene), Some(SlotState::Playing));
        (launcher, track, scene)
    }

    #[test]
    fn a_trigger_press_restarts_a_playing_clip_on_the_grid_and_a_release_does_nothing() {
        // Spec: launch-quantisation R14
        let (mut launcher, track, scene) = playing(LaunchMode::Trigger);
        let (tempo, sigs) = (tempo(), TimeSignatureMap::default());
        let at = Ticks::WHOLE + Ticks::QUARTER;
        launcher.press(
            track,
            scene,
            LaunchMode::Trigger,
            playhead(&tempo, &sigs, true, at),
        );
        assert_eq!(launcher.state_of(track, scene), Some(SlotState::Queued));
        // A second press before the line keeps the frame (R4).
        launcher.press(
            track,
            scene,
            LaunchMode::Trigger,
            playhead(&tempo, &sigs, true, at),
        );
        launcher.release(
            track,
            scene,
            LaunchMode::Trigger,
            playhead(&tempo, &sigs, true, at),
        );
        let line = frame(Ticks::WHOLE * 2);
        assert_eq!(launcher.next_due(Frames::ZERO), Some(line));
        launcher.apply_due(line);
        assert_eq!(launcher.state_of(track, scene), Some(SlotState::Playing));
        let mut playing = Vec::with_capacity(4);
        launcher.playback(&mut playing);
        assert_eq!(
            playing[0].frame, line,
            "the clip plays again from its start"
        );
        // A press while its stop is queued restarts it as well.
        launcher.stop(Some(track), playhead(&tempo, &sigs, true, Ticks::WHOLE * 2));
        assert_eq!(
            launcher.state_of(track, scene),
            Some(SlotState::QueuedToStop)
        );
        launcher.press(
            track,
            scene,
            LaunchMode::Trigger,
            playhead(&tempo, &sigs, true, Ticks::WHOLE * 2),
        );
        assert_eq!(launcher.state_of(track, scene), Some(SlotState::Queued));
    }

    #[test]
    fn a_gate_release_stops_the_clip_on_the_next_grid_line() {
        // Spec: launch-quantisation R15
        let (mut launcher, track, scene) = playing(LaunchMode::Gate);
        let (tempo, sigs) = (tempo(), TimeSignatureMap::default());
        let at = Ticks::WHOLE + Ticks::QUARTER;
        launcher.release(
            track,
            scene,
            LaunchMode::Gate,
            playhead(&tempo, &sigs, true, at),
        );
        assert_eq!(
            launcher.state_of(track, scene),
            Some(SlotState::QueuedToStop)
        );
        let line = frame(Ticks::WHOLE * 2);
        assert_eq!(launcher.next_due(Frames::ZERO), Some(line));
        launcher.apply_due(line);
        assert_eq!(launcher.state_of(track, scene), None);
        // With quantisation off the clip stops at the release frame.
        let (mut launcher, track, scene) = playing(LaunchMode::Gate);
        launcher.set_settings(LaunchSettings {
            quantization: LaunchQuantization::None,
            enabled: true,
        });
        launcher.release(
            track,
            scene,
            LaunchMode::Gate,
            playhead(&tempo, &sigs, true, at),
        );
        assert_eq!(launcher.next_due(Frames::ZERO), Some(frame(at)));
        launcher.apply_due(frame(at));
        assert_eq!(launcher.state_of(track, scene), None);
    }

    #[test]
    fn a_release_inside_one_interval_never_cancels_the_launch() {
        // Spec: launch-quantisation R16
        let (track, scene) = ids();
        let (tempo, sigs) = (tempo(), TimeSignatureMap::default());
        let mut launcher = SessionLauncher::new(16);
        let press = Ticks::QUARTER;
        launcher.press(
            track,
            scene,
            LaunchMode::Gate,
            playhead(&tempo, &sigs, true, press),
        );
        launcher.release(
            track,
            scene,
            LaunchMode::Gate,
            playhead(&tempo, &sigs, true, press + Ticks::QUARTER),
        );
        assert_eq!(launcher.state_of(track, scene), Some(SlotState::Queued));
        launcher.apply_due(frame(Ticks::WHOLE));
        assert_eq!(
            launcher.state_of(track, scene),
            Some(SlotState::QueuedToStop)
        );
        let mut playing = Vec::with_capacity(4);
        launcher.playback(&mut playing);
        assert_eq!(playing[0].frame, frame(Ticks::WHOLE));
        assert_eq!(
            launcher.next_due(frame(Ticks::WHOLE)),
            Some(frame(Ticks::WHOLE * 2)),
            "it plays one quantum"
        );
        launcher.apply_due(frame(Ticks::WHOLE * 2));
        assert_eq!(launcher.state_of(track, scene), None);
    }

    #[test]
    fn a_toggle_press_alternates_the_stop_and_the_restart() {
        // Spec: launch-quantisation R17
        let (mut launcher, track, scene) = playing(LaunchMode::Toggle);
        let (tempo, sigs) = (tempo(), TimeSignatureMap::default());
        let at = Ticks::WHOLE + Ticks::QUARTER;
        let line = frame(Ticks::WHOLE * 2);
        launcher.release(
            track,
            scene,
            LaunchMode::Toggle,
            playhead(&tempo, &sigs, true, at),
        );
        assert_eq!(
            launcher.state_of(track, scene),
            Some(SlotState::Playing),
            "no release rule"
        );
        launcher.press(
            track,
            scene,
            LaunchMode::Toggle,
            playhead(&tempo, &sigs, true, at),
        );
        assert_eq!(
            launcher.state_of(track, scene),
            Some(SlotState::QueuedToStop)
        );
        assert_eq!(launcher.next_due(Frames::ZERO), Some(line));
        // A third press replaces the stop with a restart on the line.
        launcher.press(
            track,
            scene,
            LaunchMode::Toggle,
            playhead(&tempo, &sigs, true, at),
        );
        assert_eq!(launcher.state_of(track, scene), Some(SlotState::Queued));
        launcher.apply_due(line);
        let mut playing = Vec::with_capacity(4);
        launcher.playback(&mut playing);
        assert_eq!(playing[0].frame, line);
        // A stop that lands ends the clip.
        launcher.press(
            track,
            scene,
            LaunchMode::Toggle,
            playhead(&tempo, &sigs, true, Ticks::WHOLE * 2),
        );
        launcher.apply_due(frame(Ticks::WHOLE * 3));
        assert_eq!(launcher.state_of(track, scene), None);
    }

    #[test]
    fn a_held_repeat_restarts_every_step_and_a_release_ends_the_quantum() {
        // Spec: launch-quantisation R18
        let (track, scene) = ids();
        let (tempo, sigs) = (tempo(), TimeSignatureMap::default());
        let mut launcher = SessionLauncher::new(16);
        launcher.set_settings(LaunchSettings {
            quantization: LaunchQuantization::Quarter,
            enabled: true,
        });
        let on = |at| playhead(&tempo, &sigs, true, at);
        launcher.press(track, scene, LaunchMode::Repeat, on(Ticks::ZERO));
        for step in 1..=3 {
            let line = frame(Ticks::QUARTER * step);
            assert_eq!(
                launcher.next_due(frame(Ticks::QUARTER * (step - 1))),
                Some(line)
            );
            launcher.apply_due(line);
            launcher.repeat(on(Ticks::QUARTER * step));
            let mut playing = Vec::with_capacity(4);
            launcher.playback(&mut playing);
            assert_eq!(playing[0].frame, line, "the clip restarts on line {step}");
        }
        // Released inside the quantum that began at the third line: the
        // queued retrigger becomes the stop.
        let release = Ticks::QUARTER * 3 + Ticks::SIXTEENTH;
        launcher.release(track, scene, LaunchMode::Repeat, on(release));
        assert_eq!(
            launcher.state_of(track, scene),
            Some(SlotState::QueuedToStop)
        );
        assert_eq!(
            launcher.next_due(frame(Ticks::QUARTER * 3)),
            Some(frame(Ticks::QUARTER * 4))
        );
        launcher.apply_due(frame(Ticks::QUARTER * 4));
        assert_eq!(launcher.state_of(track, scene), None);
        // With quantisation off Repeat acts as Trigger: no retriggers.
        launcher.set_settings(LaunchSettings {
            quantization: LaunchQuantization::None,
            enabled: true,
        });
        launcher.press(track, scene, LaunchMode::Repeat, on(Ticks::QUARTER * 5));
        launcher.apply_due(frame(Ticks::QUARTER * 5));
        launcher.repeat(on(Ticks::QUARTER * 5));
        assert_eq!(launcher.state_of(track, scene), Some(SlotState::Playing));
        assert_eq!(launcher.next_due(Frames::ZERO), None);
    }

    #[test]
    fn a_scene_launch_acts_as_a_trigger_press_whatever_the_mode() {
        // Spec: launch-quantisation R19
        let (mut launcher, track, scene) = playing(LaunchMode::Toggle);
        let (tempo, sigs) = (tempo(), TimeSignatureMap::default());
        let slots = [(scene, track, LaunchMode::Toggle)];
        launcher.launch_scene(
            scene,
            &slots,
            playhead(&tempo, &sigs, true, Ticks::WHOLE + Ticks::QUARTER),
        );
        assert_eq!(
            launcher.state_of(track, scene),
            Some(SlotState::Queued),
            "a restart, not a stop"
        );
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
            command_from_args(
                "session.release",
                serde_json::json!({ "track": track, "scene": scene })
            ),
            Ok(SessionCommand::Release { track, scene })
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
