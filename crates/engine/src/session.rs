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

mod follow;

use core::fmt;

use bl_commands::Context::{self, Global, Session};
use bl_commands::{CommandSpec, NoArgs, Permission, RegisterError, Registry};
use bl_core::time::{Bpm, Frames, SampleRate, TempoMap, Ticks, TimeSignature, TimeSignatureMap};
use bl_model::{LaunchMode, LaunchQuantization, LaunchSettings, SceneId, TrackId};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use follow::FollowState;
pub(crate) use follow::FollowTable;

/// How many launched scenes can wait for their frame at once (WP-3.5).
const SCENE_APPLIES: usize = 8;

/// What a slot is doing (WP-3.2, WP-3.11).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotState {
    /// A launch or a stop is waiting for the grid.
    Queued,
    /// The slot's clip plays.
    Playing,
    /// A stop is waiting for the grid.
    QueuedToStop,
    /// A Session recording goes into the slot, or waits for the grid to
    /// start (WP-3.11).
    Recording,
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
    /// The song frame its clip launched on, while it plays: where an overdub
    /// finds the clip's own time (WP-3.11).
    pub started: Option<Frames>,
    /// The clip time it had on that frame (launch-quantisation R29).
    pub offset: ClipOffset,
}

impl SlotStatus {
    /// The song frame the clip would have launched on to have the clip time
    /// it has now with no offset: its launch frame moved back by its offset,
    /// for what counts a clip's own time from its start, as an overdub does
    /// (R29, R33). `None` while the clip isn't playing.
    #[must_use]
    pub fn origin(&self, tempo: &TempoMap, rate: SampleRate) -> Option<Frames> {
        let started = self.started?;
        if self.offset.ticks == Ticks::ZERO {
            return Some(started);
        }
        let launched = tempo.frames_to_ticks(started, rate);
        Some(tempo.ticks_to_frames(launched - self.offset.ticks, rate))
    }
}

/// How far into its own time a Session clip was on its launch frame: what a
/// Legato start or a nudge moved it by (the launch quantisation spec, R27 to
/// R33). The clip time at any later song frame is this plus the time since
/// the launch frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClipOffset {
    /// In ticks, which a MIDI clip, a warped audio clip and its envelopes
    /// count by.
    pub ticks: Ticks,
    /// In frames, which an unwarped audio clip counts by (the seconds it
    /// reaches at the tempo of the frame the jump was made on).
    pub frames: i64,
}

impl ClipOffset {
    /// No offset: the clip plays from its start.
    pub const ZERO: Self = Self {
        ticks: Ticks::ZERO,
        frames: 0,
    };
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
    /// The clip time it had on that frame.
    pub offset: ClipOffset,
    /// For a taken-over track with nothing playing, the clip that stopped on
    /// it last: its Automation envelopes hold the value they had there
    /// until Back to Arrangement (WP-3.12).
    pub last: Option<StoppedClip>,
}

/// A Session clip that stopped on a track that stays taken over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoppedClip {
    /// The scene whose slot it was.
    pub scene: SceneId,
    /// The song frame it launched on.
    pub launched: Frames,
    /// The clip time it had on that frame.
    pub offset: ClipOffset,
    /// The song frame it stopped on.
    pub stopped: Frames,
}

/// A Session clip starting or ending, as an Arrangement recording sees it
/// (WP-3.13): the launcher logs one for every start, restart, stop and
/// return while a take runs, and a take that begins with clips playing
/// logs a start for each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionEvent {
    /// The track whose slot it is.
    pub track: TrackId,
    /// The scene whose slot it is.
    pub scene: SceneId,
    /// The song frame it happened on.
    pub at: Frames,
    /// For a start, the song frame the clip launched on: `at`, or earlier
    /// for a clip already playing when the take began. `None` for an end.
    pub launched: Option<Frames>,
    /// For a start, the clip time the clip had on `launched`: not zero for a
    /// Legato start or after a nudge (launch-quantisation R33).
    pub offset: ClipOffset,
}

/// The tempo and time signature a launched scene set, which hold until
/// Re-Enable Automation or Back to Arrangement (WP-3.5). Each names the
/// scene it came from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SceneOverride {
    /// The tempo, in place of the set's tempo map.
    pub tempo: Option<(SceneId, Bpm)>,
    /// The time signature, in place of the set's time-signature map.
    pub signature: Option<(SceneId, TimeSignature)>,
}

impl SceneOverride {
    /// Whether neither is set.
    #[must_use]
    pub fn is_none(&self) -> bool {
        self.tempo.is_none() && self.signature.is_none()
    }
}

/// The maps a scene's tempo and signature put in place when it launches,
/// made with the program so that switching to them allocates nothing.
pub(crate) struct SceneMaps {
    pub(crate) scene: SceneId,
    pub(crate) tempo: Option<(Bpm, TempoMap)>,
    pub(crate) signature: Option<(TimeSignature, TimeSignatureMap)>,
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
    /// Moves the playing clip of a slot by `steps` steps of the global launch
    /// quantisation at once, negative backward (the launch quantisation
    /// spec, R30 to R34).
    Nudge {
        /// The track whose slot it is.
        track: TrackId,
        /// The scene whose slot it is.
        scene: SceneId,
        /// How many steps, from −64 to 64 and not 0.
        steps: i32,
    },
    /// Launches every clip of a scene, all on one grid line.
    LaunchScene(SceneId),
    /// Stops a track's playing clip on the next grid line.
    StopTrack(TrackId),
    /// Stops every track's playing clip on the next grid line.
    StopAll,
    /// Drops what the last launch of a scene queued and hasn't played yet:
    /// its clips' starts and restarts, the stops it queued on their tracks,
    /// and its tempo and time signature (WP-6.56).
    CancelSceneLaunch(SceneId),
    /// Returns every track to the Arrangement at once (WP-3.6).
    BackToArrangement,
    /// Returns one track to the Arrangement at once (WP-3.6).
    TrackBackToArrangement(TrackId),
    /// Sets the global launch quantisation and its switch.
    SetQuantization(LaunchSettings),
    /// Starts the Session recording whose plan ([`RecordingPlan::session`])
    /// the engine holds: on the next grid line while the song plays, and
    /// from the playhead, after any count-in, while it's stopped, which
    /// starts the transport (WP-3.11).
    ///
    /// [`RecordingPlan::session`]: crate::RecordingPlan::session
    Record,
    /// Ends the Session recording on the next grid line: the new clips it
    /// made play on as loops from there (WP-3.11).
    StopRecording,
    /// Moves every playing clip whose track has a clip in this scene to that
    /// scene's slot, playing on without a break: what Capture and Insert
    /// Scene does once the scene holds copies of the playing clips (WP-3.5).
    MoveToScene(SceneId),
    /// Gives back the set's tempo and time signature in place of the ones a
    /// launched scene set: what Re-Enable Automation and Back to
    /// Arrangement do (WP-3.5).
    ClearSceneOverride,
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

/// What a press or a release of a slot's launch button reads from its clip,
/// from the running program (R22, R25, R34): a slot with no clip reads the
/// default, which acts as a Trigger on the global grid.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Button {
    pub(crate) mode: LaunchMode,
    pub(crate) legato: bool,
    /// The clip's own quantisation; `None` follows the global one.
    pub(crate) quantization: Option<LaunchQuantization>,
}

/// How long one pass of a clip's loop is, for a nudge back before its start
/// (R32): in ticks for a MIDI or warped clip, in seconds for an unwarped one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum LoopLength {
    Ticks(Ticks),
    Seconds(f64),
}

/// One slot that holds a clip, in the running program: what a scene launch
/// queues, what a press reads (R34) and what a nudge needs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SlotRow {
    pub(crate) scene: SceneId,
    pub(crate) track: TrackId,
    pub(crate) button: Button,
    /// The loop's length while the clip's Loop switch is on.
    pub(crate) looping: Option<LoopLength>,
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
    /// than a stop (the launch quantisation spec, R17).
    restart: bool,
    /// A stop to queue once the launch or restart at `due` has happened:
    /// a Gate or Repeat button was released before the clip started (R19).
    then_stop: Option<Frames>,
    /// The grid tick the last launch or restart was queued for, which the
    /// stop after it and Repeat's next line are counted from.
    line: Ticks,
    /// Whether a Repeat button is held, so the clip restarts on each line
    /// (R21).
    repeating: bool,
    /// Whether the queued restart is one Repeat queued itself, which a
    /// release turns into the stop (R21).
    auto: bool,
    /// Where the clip's follow action is (WP-3.10).
    follow: FollowState,
    /// The clip's own launch quantisation as the last press or release read
    /// it, which Repeat's restarts are counted on (R21, R25).
    quantization: Option<LaunchQuantization>,
    /// Whether the clip, as the last press read it, takes over the clip time
    /// of the clip playing on the track when it launches or restarts (R27,
    /// R28).
    legato: bool,
    /// The grid tick the clip last started on, which its clip time is
    /// counted from (zero for a clip whose launch tick is unknown).
    tick: Ticks,
    /// The clip time it had on that tick (R27 to R33).
    offset: ClipOffset,
    /// The clip time the launch or restart waiting at `due` starts at: the
    /// clip time of the clip it takes over from (R27), worked out before
    /// that clip is removed.
    takeover: ClipOffset,
}

/// The follow action of the scene launched last, which runs from the line it
/// launched on (WP-3.10).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SceneRun {
    scene: SceneId,
    line: Ticks,
    follow: FollowState,
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

    /// The clip time the playing clip has on `tick` and song frame `frame`
    /// (R27): what it started with plus the time since it started.
    fn clip_time_at(self, tick: Ticks, frame: Frames) -> ClipOffset {
        let since = self
            .started
            .map_or(0, |started| (frame.get() - started.get()).max(0));
        ClipOffset {
            ticks: self.offset.ticks + (tick - self.tick).max(Ticks::ZERO),
            frames: self.offset.frames + since,
        }
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
    /// Launched scenes whose tempo and signature wait for their launch
    /// frame (WP-3.5), with that frame.
    scenes: Vec<(SceneId, Frames)>,
    /// The tracks a Session clip has taken over from the Arrangement, until
    /// a return (WP-3.6). Capacity as `slots`: a track is listed once.
    overridden: Vec<TrackId>,
    /// The clip that stopped last on each taken-over track (WP-3.12). Capacity
    /// as `overridden`.
    stopped: Vec<(TrackId, StoppedClip)>,
    /// Stops that take a track over on a grid line without a Session clip
    /// playing on it: a stop button pressed on an idle track silences its
    /// Arrangement there.
    marks: Vec<(TrackId, Frames)>,
    /// The starts and ends since the processor last took them (WP-3.13).
    /// Room for two per slot and two more per track, drained after every
    /// change; an event past its end counts in `overflowed`.
    pub(crate) log: Vec<SessionEvent>,
    /// Launches dropped because every slot was taken.
    pub(crate) overflowed: u64,
    /// The follow action of the scene launched last (WP-3.10).
    scene_run: Option<SceneRun>,
}

impl SessionLauncher {
    /// A launcher with the default quantisation, with room for `capacity`
    /// slots.
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            settings: LaunchSettings::default(),
            slots: Vec::with_capacity(capacity),
            scenes: Vec::with_capacity(SCENE_APPLIES),
            overridden: Vec::with_capacity(capacity),
            stopped: Vec::with_capacity(capacity),
            marks: Vec::with_capacity(capacity),
            log: Vec::with_capacity(capacity * 4),
            overflowed: 0,
            scene_run: None,
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
        self.scenes.clear();
        self.overridden.clear();
        self.stopped.clear();
        self.marks.clear();
        self.scene_run = None;
    }

    /// The first line after `from` of the grid a clip with `quantization`
    /// places its presses on: its own step, or the global value and switch
    /// while it follows them (R25). The one place the grid is worked out.
    fn next_line(
        &self,
        quantization: Option<LaunchQuantization>,
        from: Ticks,
        signatures: &TimeSignatureMap,
    ) -> Ticks {
        match quantization {
            Some(step) => step.next_line(from, signatures),
            None => self.settings.next_line(from, signatures),
        }
    }

    /// The tick of the grid line a launch or a stop asked for now waits for:
    /// the next line of the grid of a clip with `quantization` (`None` for
    /// the global grid), at once while the song is stopped or quantisation
    /// is off.
    fn line_for(&self, quantization: Option<LaunchQuantization>, playhead: Playhead<'_>) -> Ticks {
        if !playhead.wait_for_grid {
            return playhead.position;
        }
        self.next_line(quantization, playhead.position, playhead.signatures)
    }

    /// The frame a stop or a scene launch asked for now waits for, on the
    /// global grid (R26).
    pub(crate) fn frame_for(&self, playhead: Playhead<'_>) -> Frames {
        playhead.frames(self.line_for(None, playhead))
    }

    /// Whether Repeat has a step to repeat at: with quantisation off it acts
    /// as Trigger (R21).
    fn steps(&self, quantization: Option<LaunchQuantization>) -> bool {
        match quantization {
            Some(step) => step != LaunchQuantization::None,
            None => self.settings.enabled && self.settings.quantization != LaunchQuantization::None,
        }
    }

    /// A press of the launch button of `(track, scene)`, whose clip is
    /// read as `button` says: what it does is the mode's rule (R12 to R21),
    /// on the clip's own grid (R25).
    pub(crate) fn press(
        &mut self,
        track: TrackId,
        scene: SceneId,
        button: Button,
        playhead: Playhead<'_>,
    ) {
        let line = self.line_for(button.quantization, playhead);
        let at = playhead.frames(line);
        self.press_at(track, scene, button, playhead.wait_for_grid, line, at);
    }

    /// A release of the launch button of `(track, scene)`, whose clip is
    /// read as `button` says. Only Gate and Repeat act on it (R18, R21), on
    /// the clip's own grid.
    pub(crate) fn release(
        &mut self,
        track: TrackId,
        scene: SceneId,
        button: Button,
        playhead: Playhead<'_>,
    ) {
        if !matches!(button.mode, LaunchMode::Gate | LaunchMode::Repeat) {
            return;
        }
        let settings = self.settings;
        let at = playhead.frames(self.line_for(button.quantization, playhead));
        let Some(slot) = self.find_mut(track, scene) else {
            return;
        };
        slot.repeating = false;
        if slot.auto && slot.restart && slot.due.is_some() {
            // The clip plays out the quantum it is in (R21).
            slot.stop_at(at);
        } else if slot.started.is_none() || slot.restart {
            // The release never cancels a queued launch: the stop lands on
            // the line after it, so the clip plays one quantum (R19).
            let after = match button.quantization {
                Some(step) => step.next_line(slot.line, playhead.signatures),
                None => settings.next_line(slot.line, playhead.signatures),
            };
            slot.then_stop = Some(playhead.frames(after));
        } else {
            // Keep the earlier of a stop already queued and this one.
            slot.stop_at(slot.due.map_or(at, |due| due.min(at)));
        }
    }

    /// Launches every slot of `scene` that `slots` names (from the running
    /// program), all on one frame of the global grid: a scene's clips start
    /// together (observed in Live 12.4.3), whatever their own quantisation
    /// (R26). A scene launch acts as a Trigger press on each, whatever the
    /// clip's mode, and a Legato clip still takes over (R28). Returns the
    /// frame they start on, which is where the scene's own tempo and
    /// signature apply.
    pub(crate) fn launch_scene(
        &mut self,
        scene: SceneId,
        slots: &[SlotRow],
        playhead: Playhead<'_>,
    ) -> Frames {
        let line = self.line_for(None, playhead);
        let at = playhead.frames(line);
        for row in slots {
            let button = Button {
                mode: LaunchMode::Trigger,
                ..row.button
            };
            self.press_at(row.track, scene, button, playhead.wait_for_grid, line, at);
        }
        self.start_scene_run(scene, line);
        at
    }

    /// The frame a launch asked for now lands on, for a scene with no clips
    /// to launch.
    pub(crate) fn landing(&self, playhead: Playhead<'_>) -> Frames {
        self.frame_for(playhead)
    }

    /// Moves the playing slot of each track that has a slot in `slots`
    /// (those of the destination scene) to that scene, keeping when it
    /// started and any stop it waits for. A slot still waiting to start
    /// stays where it is.
    pub(crate) fn move_to_scene(&mut self, scene: SceneId, slots: &[SlotRow]) {
        for slot in &mut self.slots {
            if slot.started.is_some() && slots.iter().any(|row| row.track == slot.track) {
                slot.scene = scene;
            }
        }
    }

    /// Holds `scene`'s tempo and signature for `due`. A later launch of the
    /// same scene replaces an earlier one; a full list drops the launch.
    pub(crate) fn queue_scene(&mut self, scene: SceneId, due: Frames) {
        self.scenes.retain(|&(queued, _)| queued != scene);
        if self.scenes.len() < self.scenes.capacity() {
            self.scenes.push((scene, due));
        } else {
            self.overflowed += 1;
        }
    }

    /// The scene whose launch frame has come at or before `at`, if any,
    /// oldest first.
    pub(crate) fn take_due_scene(&mut self, at: Frames) -> Option<SceneId> {
        let index = (0..self.scenes.len())
            .filter(|&index| self.scenes[index].1 <= at)
            .min_by_key(|&index| self.scenes[index].1)?;
        Some(self.scenes.remove(index).0)
    }

    /// Moves every frame the launcher holds to where `convert` puts it: the
    /// tempo map changed, and each frame keeps its beat.
    pub(crate) fn remap(&mut self, convert: impl Fn(Frames) -> Frames) {
        for slot in &mut self.slots {
            slot.started = slot.started.map(&convert);
        }
        for (_, clip) in &mut self.stopped {
            clip.launched = convert(clip.launched);
            clip.stopped = convert(clip.stopped);
        }
        self.remap_pending(convert);
    }

    /// Moves the frames the launcher waits for to where `convert` puts them,
    /// leaving those of clips that started: a queued launch, restart or stop,
    /// a scene's tempo, a takeover and a follow action.
    pub(crate) fn remap_pending(&mut self, convert: impl Fn(Frames) -> Frames) {
        for slot in &mut self.slots {
            slot.due = slot.due.map(&convert);
            slot.then_stop = slot.then_stop.map(&convert);
            if let FollowState::Armed { due, .. } = &mut slot.follow {
                *due = convert(*due);
            }
        }
        if let Some(SceneRun {
            follow: FollowState::Armed { due, .. },
            ..
        }) = &mut self.scene_run
        {
            *due = convert(*due);
        }
        for (_, due) in &mut self.scenes {
            *due = convert(*due);
        }
        for (_, due) in &mut self.marks {
            *due = convert(*due);
        }
    }

    /// Launches `(track, scene)` to play from `at`, already worked out: a
    /// recorded slot that plays on from where its recording ended (WP-3.11).
    /// A slot the launcher already holds is left alone. It starts in the
    /// block it is asked in, so no release reads its grid line first.
    pub(crate) fn launch_at(&mut self, track: TrackId, scene: SceneId, at: Frames) {
        if self.find_mut(track, scene).is_none() {
            self.press_at(track, scene, Button::default(), false, Ticks::ZERO, at);
            // The launch tick is unknown, so a recorded clip follows nothing.
            if let Some(slot) = self.find_mut(track, scene) {
                slot.follow = FollowState::Spent;
            }
        }
    }

    /// A press of `(track, scene)`'s launch button with its grid line already
    /// worked out, so a scene's slots share one frame. A Legato clip takes
    /// over only `while_playing`: with the transport stopped it starts from
    /// its start (R27).
    fn press_at(
        &mut self,
        track: TrackId,
        scene: SceneId,
        button: Button,
        while_playing: bool,
        line: Ticks,
        at: Frames,
    ) {
        let Button {
            mode,
            legato,
            quantization,
        } = button;
        let legato = legato && while_playing;
        let repeating = mode == LaunchMode::Repeat && self.steps(quantization);
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
                follow: FollowState::Unarmed,
                quantization,
                legato,
                tick: Ticks::ZERO,
                offset: ClipOffset::ZERO,
                takeover: ClipOffset::ZERO,
            });
            return;
        };
        slot.then_stop = None;
        slot.auto = false;
        slot.repeating = repeating;
        slot.quantization = quantization;
        slot.legato = legato;
        if slot.started.is_none() {
            // Still queued to start: it keeps the frame it was queued for
            // (R4), and a second press changes nothing.
            return;
        }
        let queued_stop = slot.due.is_some() && !slot.restart;
        if mode == LaunchMode::Toggle && !queued_stop {
            // Toggle: the press after a start queues the stop (R20).
            slot.stop_at(at);
        } else if !(slot.due.is_some() && slot.restart) {
            // Trigger, Gate and Repeat restart the clip on the grid; so does
            // a Toggle press while its stop is queued (R17, R20).
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

    /// Drops what the last launch of `scene` queued and hasn't happened yet
    /// (the session context menus spec, R10): a clip of the scene still
    /// waiting to start is forgotten, one waiting to restart plays on, a stop
    /// that launch queued on a track's other clip (the same frame) is
    /// dropped, and the scene's tempo and signature never apply. Clips
    /// playing or stopped stay as they are.
    pub(crate) fn cancel_scene_launch(&mut self, scene: SceneId, slots: &[SlotRow]) {
        for row in slots {
            let track = row.track;
            let Some(due) = self
                .slots
                .iter()
                .find(|slot| slot.track == track && slot.scene == scene)
                .and_then(|slot| slot.due)
            else {
                continue;
            };
            for slot in &mut self.slots {
                if slot.track != track {
                    continue;
                }
                if slot.scene == scene {
                    if slot.started.is_some() {
                        slot.due = None;
                        slot.restart = false;
                        slot.then_stop = None;
                        slot.auto = false;
                        slot.repeating = false;
                    }
                } else if slot.started.is_some() && !slot.restart && slot.due == Some(due) {
                    slot.due = None;
                }
            }
        }
        self.slots
            .retain(|slot| slot.scene != scene || slot.started.is_some());
        self.scenes.retain(|&(queued, _)| queued != scene);
        if self.scene_run.is_some_and(|run| run.scene == scene) {
            self.scene_run = None;
        }
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
            None => {
                tracks.iter().for_each(|&track| self.mark(track, at));
                self.scene_run = None;
            }
        }
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

    /// Logs a clip starting (`launched`) or ending at `at` (WP-3.13).
    fn note(
        &mut self,
        track: TrackId,
        scene: SceneId,
        at: Frames,
        launched: Option<Frames>,
        offset: ClipOffset,
    ) {
        if self.log.len() < self.log.capacity() {
            self.log.push(SessionEvent {
                track,
                scene,
                at,
                launched,
                offset,
            });
        } else {
            self.overflowed += 1;
        }
    }

    /// The slots whose clips play, with the frame each launched on and the
    /// clip time it had there.
    pub(crate) fn started(
        &self,
    ) -> impl Iterator<Item = (TrackId, SceneId, Frames, ClipOffset)> + '_ {
        self.slots
            .iter()
            .filter_map(|slot| Some((slot.track, slot.scene, slot.started?, slot.offset)))
    }

    /// Notes the clip that stopped on `track`, replacing the one before.
    fn remember_stop(&mut self, clip: StoppedClip, track: TrackId) {
        if let Some((_, last)) = self.stopped.iter_mut().find(|(t, _)| *t == track) {
            *last = clip;
        } else if self.stopped.len() < self.stopped.capacity() {
            self.stopped.push((track, clip));
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
    pub(crate) fn back_to_arrangement(&mut self, track: Option<TrackId>, at: Frames) {
        let returned = |t: TrackId| track.is_none_or(|only| only == t);
        for i in 0..self.slots.len() {
            let slot = self.slots[i];
            if returned(slot.track) && slot.started.is_some() {
                self.note(slot.track, slot.scene, at, None, ClipOffset::ZERO);
            }
        }
        self.slots.retain(|slot| !returned(slot.track));
        if track.is_none() {
            self.scene_run = None;
        }
        self.marks.retain(|&(t, _)| !returned(t));
        self.overridden.retain(|&t| !returned(t));
        self.stopped.retain(|&(t, _)| !returned(t));
    }

    /// Forgets the takeovers of tracks the running program no longer has,
    /// so a deleted track doesn't keep Back to Arrangement lit.
    pub(crate) fn retain_tracks(&mut self, tracks: &[TrackId]) {
        self.overridden.retain(|t| tracks.contains(t));
        self.stopped.retain(|(t, _)| tracks.contains(t));
        self.marks.retain(|(t, _)| tracks.contains(t));
    }

    /// The tracks a Session clip has taken over, for the engine to publish.
    pub(crate) fn overridden(&self) -> &[TrackId] {
        &self.overridden
    }

    /// Applies every change due at or before the playhead `at`, which the
    /// processor calls at a block part's start.
    pub(crate) fn apply_due(&mut self, at: Frames) {
        self.work_out_takeovers(at);
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
            let (track, scene) = (slot.track, slot.scene);
            let (running, launching) = (
                slot.started.is_some(),
                slot.started.is_none() || slot.restart,
            );
            if running {
                self.note(track, scene, due, None, ClipOffset::ZERO);
            }
            if launching {
                // The queued launch or restart plays from the frame it was
                // queued for, and takes its track over from the Arrangement
                // there; a stop queued behind it follows.
                let offset = self.slots[i].takeover;
                self.note(track, scene, due, Some(due), offset);
                let slot = &mut self.slots[i];
                slot.started = Some(due);
                slot.tick = slot.line;
                slot.offset = offset;
                slot.takeover = ClipOffset::ZERO;
                slot.restart = false;
                slot.auto = false;
                slot.follow = FollowState::Unarmed;
                let then_stop = slot.then_stop.take();
                if let Some(stop) = then_stop {
                    slot.due = Some(stop);
                } else {
                    i += 1;
                }
                self.take_over(track);
            } else {
                // A queued stop: the clip is done.
                let slot = self.slots.swap_remove(i);
                if let Some(launched) = slot.started {
                    self.remember_stop(
                        StoppedClip {
                            scene: slot.scene,
                            launched,
                            offset: slot.offset,
                            stopped: due,
                        },
                        slot.track,
                    );
                }
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

    /// Works out the clip time each launch or restart due at or before `at`
    /// starts at: zero, or with Legato on the clip time that the clip
    /// playing on its track (itself, for a restart) has reached on the
    /// launch frame (R27, R28). It reads the playing clip before
    /// [`Self::apply_due`] removes it.
    fn work_out_takeovers(&mut self, at: Frames) {
        for i in 0..self.slots.len() {
            let slot = self.slots[i];
            let Some(due) = slot.due.filter(|&due| due <= at) else {
                continue;
            };
            if slot.started.is_some() && !slot.restart {
                continue;
            }
            let donor = if slot.started.is_some() {
                Some(slot)
            } else {
                self.slots
                    .iter()
                    .copied()
                    .find(|other| other.track == slot.track && other.started.is_some())
            };
            self.slots[i].takeover = match donor.filter(|_| slot.legato) {
                Some(donor) => donor.clip_time_at(slot.line, due),
                None => ClipOffset::ZERO,
            };
        }
    }

    /// Queues the next restart of every slot whose Repeat button is held and
    /// whose clip plays, one quantisation step after the last, on the clip's
    /// own grid (R21, R25). The processor calls it after [`Self::apply_due`].
    pub(crate) fn repeat(&mut self, playhead: Playhead<'_>) {
        let settings = self.settings;
        let global = settings.enabled && settings.quantization != LaunchQuantization::None;
        for slot in &mut self.slots {
            let steps = slot
                .quantization
                .map_or(global, |step| step != LaunchQuantization::None);
            if steps && slot.repeating && slot.started.is_some() && slot.due.is_none() {
                slot.line = match slot.quantization {
                    Some(step) => step.next_line(slot.line, playhead.signatures),
                    None => settings.next_line(slot.line, playhead.signatures),
                };
                slot.due = Some(playhead.frames(slot.line));
                slot.restart = true;
                slot.auto = true;
            }
        }
    }

    /// Moves the playing clip of `(track, scene)` by `steps` steps of the
    /// global launch quantisation, at once (R30 to R32): its clip time grows
    /// by the steps, and where a nudge back would fall before the clip's
    /// start, a looping clip goes whole loops later and any other clip to its
    /// start. A slot that doesn't play, or is only waiting to start, changes
    /// nothing, and a launch, restart or stop already queued keeps its frame
    /// (R31, R33). `looping` is the clip's loop length while its Loop switch
    /// is on.
    pub(crate) fn nudge(
        &mut self,
        track: TrackId,
        scene: SceneId,
        steps: i32,
        looping: Option<LoopLength>,
        playhead: Playhead<'_>,
    ) {
        let settings = self.settings;
        let Some(slot) = self.find_mut(track, scene) else {
            return;
        };
        let Some(started) = slot.started else {
            return;
        };
        let now = playhead.position.max(Ticks::ZERO);
        let bar = playhead.signatures.signature_at(now).bar_length();
        let step = settings
            .enabled
            .then_some(settings.quantization)
            .and_then(|step| step.ticks(bar))
            .unwrap_or(Ticks::SIXTEENTH / 2);
        let time = slot.clip_time_at(now, playhead.frames(now));
        let target = time.ticks + step * i64::from(steps);
        let (ticks, frames) = if target >= Ticks::ZERO {
            (target, None)
        } else if let Some(length) = looping {
            let length = match length {
                LoopLength::Ticks(length) => length,
                LoopLength::Seconds(seconds) => {
                    let at = playhead.tempo.ticks_to_seconds(now);
                    playhead.tempo.seconds_to_ticks(at + seconds) - now
                }
            };
            if length <= Ticks::ZERO {
                (Ticks::ZERO, Some(0))
            } else {
                let laps = (-target.get() + length.get() - 1) / length.get();
                (target + length * laps, None)
            }
        } else {
            (Ticks::ZERO, Some(0))
        };
        let moved = ticks - time.ticks;
        let at = playhead.frames(now).get();
        let frame_time = frames.unwrap_or_else(|| {
            let to = playhead
                .tempo
                .ticks_to_frames(now + moved, playhead.rate)
                .get();
            time.frames + to - at
        });
        slot.offset = ClipOffset {
            ticks: slot.offset.ticks + moved,
            frames: slot.offset.frames + frame_time - time.frames,
        };
        let offset = slot.offset;
        self.note(track, scene, Frames(at), None, ClipOffset::ZERO);
        self.note(track, scene, Frames(at), Some(started), offset);
    }

    /// The first frame a change is due at, but after `at`: where the
    /// processor next splits its block.
    pub(crate) fn next_due(&self, at: Frames) -> Option<Frames> {
        self.slots
            .iter()
            .filter_map(|slot| slot.due)
            .chain(self.scenes.iter().map(|&(_, due)| due))
            .chain(self.marks.iter().map(|&(_, due)| due))
            .chain(self.slots.iter().filter_map(|slot| slot.follow.due()))
            .chain(self.scene_run.and_then(|run| run.follow.due()))
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
                    started: slot.started,
                    offset: slot.offset,
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
                    offset: slot.offset,
                    last: None,
                });
            }
        }
        for &track in &self.overridden {
            if out.len() < out.capacity() && !out.iter().any(|entry| entry.track == track) {
                out.push(SessionPlayback {
                    track,
                    scene: None,
                    frame: Frames::ZERO,
                    offset: ClipOffset::ZERO,
                    last: self
                        .stopped
                        .iter()
                        .find(|(t, _)| *t == track)
                        .map(|&(_, clip)| clip),
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

/// The arguments of `session.nudge`: the slot whose playing clip moves, and
/// by how many steps (the launch quantisation spec, R34).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NudgeArgs {
    /// The track whose slot it is.
    pub track: TrackId,
    /// The scene whose slot it is.
    pub scene: SceneId,
    /// How many steps of the global launch quantisation: a whole number
    /// from −64 to 64 other than 0, negative to move backward.
    #[schemars(range(min = -64, max = 64))]
    pub steps: i32,
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
        Action::Release => {
            let LaunchArgs { track, scene } = serde_json::from_value(args).map_err(invalid)?;
            Ok(SessionCommand::Release { track, scene })
        }
        Action::Nudge => {
            let NudgeArgs {
                track,
                scene,
                steps,
            } = serde_json::from_value(args).map_err(invalid)?;
            if steps == 0 || steps.unsigned_abs() > 64 {
                return Err(RunError::InvalidArgs {
                    command: entry.id,
                    message: format!(
                        "`steps` is {steps}, not a whole number from -64 to 64 other than 0"
                    ),
                });
            }
            Ok(SessionCommand::Nudge {
                track,
                scene,
                steps,
            })
        }
        Action::LaunchScene => {
            let LaunchSceneArgs { scene } = serde_json::from_value(args).map_err(invalid)?;
            Ok(SessionCommand::LaunchScene(scene))
        }
        Action::CancelSceneLaunch => {
            let LaunchSceneArgs { scene } = serde_json::from_value(args).map_err(invalid)?;
            Ok(SessionCommand::CancelSceneLaunch(scene))
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
    /// [`SessionCommand::Release`].
    Release,
    /// [`SessionCommand::Nudge`].
    Nudge,
    /// [`SessionCommand::LaunchScene`].
    LaunchScene,
    /// [`SessionCommand::CancelSceneLaunch`].
    CancelSceneLaunch,
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
            Action::Launch | Action::Release => spec.args::<LaunchArgs>(),
            Action::Nudge => spec.args::<NudgeArgs>(),
            Action::LaunchScene | Action::CancelSceneLaunch => spec.args::<LaunchSceneArgs>(),
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
        key: None,
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
        id: "session.nudge",
        action: Action::Nudge,
        label: "Nudge Clip",
        contexts: &[Session],
        key: None,
        description: "Moves the playing clip in the slot at `track` and `scene` by `steps` steps \
            of the global launch quantisation, as the Launch Properties panel's Nudge Backward \
            and Nudge Forward buttons do, at once and not on the grid: a negative `steps` moves \
            it back, from -64 to 64 and never 0. A step is the length of the global value at \
            the playhead, or a thirty-second note with the value None or the switch off. \
            Forward, the clip goes on round its loop as if it had played that much longer. \
            Backward, where the move would fall before the clip's start, a looping clip goes the \
            fewest whole loops later and a clip without Loop goes to its start. A slot that is \
            empty, stopped or still waiting to start changes nothing, and a launch or a stop \
            already queued keeps its frame. The slot's `clip_time` in `session.state`'s \
            `slot_states` shows the move. It changes nothing in the set.",
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
        id: "session.cancel_scene_launch",
        action: Action::CancelSceneLaunch,
        label: "Cancel Scene Launch",
        contexts: &[Session],
        key: None,
        description: "Cancels the launch of the scene at `scene` that is still waiting for its \
            line of the launch quantisation grid, as the scene menu's Cancel Scene Launch \
            does: the clips of that scene waiting to start never start, one waiting to \
            restart plays on, the stops that launch queued on the clips' tracks are dropped, \
            and the scene's tempo and time signature never apply. Clips already playing keep \
            playing and stopped ones stay stopped. A launch that has happened isn't undone, \
            and with nothing of the scene waiting the command is refused. The slots' \
            states, `queued` among them, show in `session.state`'s `launches`. It changes \
            nothing in the set.",
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

    fn mode(mode: LaunchMode) -> Button {
        Button {
            mode,
            ..Button::default()
        }
    }

    fn row(scene: SceneId, track: TrackId, mode: LaunchMode) -> SlotRow {
        SlotRow {
            scene,
            track,
            button: Button {
                mode,
                ..Button::default()
            },
            looping: None,
        }
    }

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
            mode(LaunchMode::Trigger),
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
                scene: Some(scene),
                frame: due,
                offset: ClipOffset::ZERO,
                last: None,
            }]
        );
        // While stopped it plays at once.
        let (other, scene2) = ids();
        launcher.press(
            other,
            scene2,
            mode(LaunchMode::Trigger),
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
            mode(LaunchMode::Trigger),
            playhead(&tempo, &sigs, true, Ticks::ZERO),
        );
        launcher.press(
            track,
            other,
            mode(LaunchMode::Trigger),
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
            mode(LaunchMode::Trigger),
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
            mode(LaunchMode::Trigger),
            playhead(&tempo, &sigs, true, Ticks::QUARTER),
        );
        let due = tempo.ticks_to_frames(Ticks::WHOLE, SampleRate::HZ_48000);
        launcher.press(
            track,
            scene,
            mode(LaunchMode::Trigger),
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
        // Spec: launch-quantisation R17. A press of a playing Trigger clip
        // queues a restart of the same slot, so there is still one entry.
        launcher.press(
            track,
            scene,
            mode(LaunchMode::Trigger),
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
            mode(LaunchMode::Trigger),
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
            mode(LaunchMode::Trigger),
            playhead(&tempo, &sigs, true, at),
        );
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
            launcher.press(
                track,
                scene,
                mode(LaunchMode::Trigger),
                playhead(&tempo, &sigs, true, Ticks::ZERO),
            );
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
        let slots: Vec<SlotRow> = (0..3)
            .map(|_| row(scene, TrackId(alloc.allocate()), LaunchMode::Trigger))
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
        assert!(playing.iter().all(|entry| entry.scene == Some(scene)));
    }

    #[test]
    fn cancelling_a_scene_launch_drops_what_it_queued_and_leaves_what_plays() {
        // Spec: session-context-menus R10
        let tempo = tempo();
        let sigs = TimeSignatureMap::default();
        let mut launcher = SessionLauncher::new(16);
        let mut alloc = bl_core::id::IdAllocator::new();
        let (first, second) = (SceneId(alloc.allocate()), SceneId(alloc.allocate()));
        let (a, b, c) = (
            TrackId(alloc.allocate()),
            TrackId(alloc.allocate()),
            TrackId(alloc.allocate()),
        );
        let bar = tempo.ticks_to_frames(Ticks::WHOLE, SampleRate::HZ_48000);
        // Track a plays the first scene's clip, and so does c, which the
        // second scene has nothing on.
        launcher.launch_scene(
            first,
            &[
                row(first, a, LaunchMode::Trigger),
                row(first, c, LaunchMode::Trigger),
            ],
            playhead(&tempo, &sigs, false, Ticks::ZERO),
        );
        launcher.apply_due(Frames::ZERO);
        // The second scene launches before the bar line: a takes it over
        // from the first (its stop is queued), b starts, and its tempo waits.
        let slots = [
            row(second, a, LaunchMode::Trigger),
            row(second, b, LaunchMode::Trigger),
        ];
        launcher.launch_scene(
            second,
            &slots,
            playhead(&tempo, &sigs, true, Ticks::QUARTER),
        );
        launcher.queue_scene(second, bar);
        assert_eq!(launcher.state_of(a, first), Some(SlotState::QueuedToStop));
        assert_eq!(launcher.state_of(b, second), Some(SlotState::Queued));
        launcher.cancel_scene_launch(second, &slots);
        assert_eq!(launcher.state_of(a, first), Some(SlotState::Playing));
        assert_eq!(launcher.state_of(a, second), None);
        assert_eq!(launcher.state_of(b, second), None);
        assert_eq!(launcher.state_of(c, first), Some(SlotState::Playing));
        assert_eq!(
            launcher.take_due_scene(bar),
            None,
            "the tempo never applies"
        );
        launcher.apply_due(bar);
        let mut playing = Vec::with_capacity(8);
        launcher.playback(&mut playing);
        assert!(
            playing.iter().all(|entry| entry.scene == Some(first)),
            "{playing:?}"
        );
    }

    #[test]
    fn cancelling_a_scene_launch_leaves_a_restart_unqueued_and_a_launch_that_happened() {
        // Spec: session-context-menus R10
        let tempo = tempo();
        let sigs = TimeSignatureMap::default();
        let mut launcher = SessionLauncher::new(16);
        let (track, scene) = ids();
        let slots = [row(scene, track, LaunchMode::Trigger)];
        launcher.launch_scene(scene, &slots, playhead(&tempo, &sigs, false, Ticks::ZERO));
        launcher.apply_due(Frames::ZERO);
        launcher.launch_scene(scene, &slots, playhead(&tempo, &sigs, true, Ticks::QUARTER));
        assert_eq!(launcher.state_of(track, scene), Some(SlotState::Queued));
        launcher.cancel_scene_launch(scene, &slots);
        assert_eq!(launcher.state_of(track, scene), Some(SlotState::Playing));
        // Nothing queued any more: a second cancel changes nothing.
        launcher.cancel_scene_launch(scene, &slots);
        assert_eq!(launcher.state_of(track, scene), Some(SlotState::Playing));
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
            ..LaunchSettings::default()
        });
        launcher.press(
            track,
            scene,
            mode(LaunchMode::Trigger),
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
            mode(LaunchMode::Trigger),
            playhead(&tempo, &sigs, true, at),
        );
        let due = tempo.ticks_to_frames(Ticks::WHOLE * 2, SampleRate::HZ_48000);
        assert_eq!(launcher.next_due(Frames::ZERO), Some(due));
        launcher.set_settings(LaunchSettings {
            quantization: LaunchQuantization::Sixteenth,
            enabled: true,
            ..LaunchSettings::default()
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
    fn playing(launch_mode: LaunchMode) -> (SessionLauncher, TrackId, SceneId) {
        let (track, scene) = ids();
        let mut launcher = SessionLauncher::new(16);
        let (tempo, sigs) = (tempo(), TimeSignatureMap::default());
        launcher.press(
            track,
            scene,
            mode(launch_mode),
            playhead(&tempo, &sigs, true, Ticks::ZERO),
        );
        launcher.apply_due(frame(Ticks::WHOLE));
        assert_eq!(launcher.state_of(track, scene), Some(SlotState::Playing));
        (launcher, track, scene)
    }

    #[test]
    fn a_trigger_press_restarts_a_playing_clip_on_the_grid_and_a_release_does_nothing() {
        // Spec: launch-quantisation R17
        let (mut launcher, track, scene) = playing(LaunchMode::Trigger);
        let (tempo, sigs) = (tempo(), TimeSignatureMap::default());
        let at = Ticks::WHOLE + Ticks::QUARTER;
        launcher.press(
            track,
            scene,
            mode(LaunchMode::Trigger),
            playhead(&tempo, &sigs, true, at),
        );
        assert_eq!(launcher.state_of(track, scene), Some(SlotState::Queued));
        // A second press before the line keeps the frame (R4).
        launcher.press(
            track,
            scene,
            mode(LaunchMode::Trigger),
            playhead(&tempo, &sigs, true, at),
        );
        launcher.release(
            track,
            scene,
            mode(LaunchMode::Trigger),
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
        launcher.stop(
            Some(track),
            &[],
            playhead(&tempo, &sigs, true, Ticks::WHOLE * 2),
        );
        assert_eq!(
            launcher.state_of(track, scene),
            Some(SlotState::QueuedToStop)
        );
        launcher.press(
            track,
            scene,
            mode(LaunchMode::Trigger),
            playhead(&tempo, &sigs, true, Ticks::WHOLE * 2),
        );
        assert_eq!(launcher.state_of(track, scene), Some(SlotState::Queued));
    }

    #[test]
    fn a_gate_release_stops_the_clip_on_the_next_grid_line() {
        // Spec: launch-quantisation R18
        let (mut launcher, track, scene) = playing(LaunchMode::Gate);
        let (tempo, sigs) = (tempo(), TimeSignatureMap::default());
        let at = Ticks::WHOLE + Ticks::QUARTER;
        launcher.release(
            track,
            scene,
            mode(LaunchMode::Gate),
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
            ..LaunchSettings::default()
        });
        launcher.release(
            track,
            scene,
            mode(LaunchMode::Gate),
            playhead(&tempo, &sigs, true, at),
        );
        assert_eq!(launcher.next_due(Frames::ZERO), Some(frame(at)));
        launcher.apply_due(frame(at));
        assert_eq!(launcher.state_of(track, scene), None);
    }

    #[test]
    fn a_release_inside_one_interval_never_cancels_the_launch() {
        // Spec: launch-quantisation R19
        let (track, scene) = ids();
        let (tempo, sigs) = (tempo(), TimeSignatureMap::default());
        let mut launcher = SessionLauncher::new(16);
        let press = Ticks::QUARTER;
        launcher.press(
            track,
            scene,
            mode(LaunchMode::Gate),
            playhead(&tempo, &sigs, true, press),
        );
        launcher.release(
            track,
            scene,
            mode(LaunchMode::Gate),
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
        // Spec: launch-quantisation R20
        let (mut launcher, track, scene) = playing(LaunchMode::Toggle);
        let (tempo, sigs) = (tempo(), TimeSignatureMap::default());
        let at = Ticks::WHOLE + Ticks::QUARTER;
        let line = frame(Ticks::WHOLE * 2);
        launcher.release(
            track,
            scene,
            mode(LaunchMode::Toggle),
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
            mode(LaunchMode::Toggle),
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
            mode(LaunchMode::Toggle),
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
            mode(LaunchMode::Toggle),
            playhead(&tempo, &sigs, true, Ticks::WHOLE * 2),
        );
        launcher.apply_due(frame(Ticks::WHOLE * 3));
        assert_eq!(launcher.state_of(track, scene), None);
    }

    #[test]
    fn a_held_repeat_restarts_every_step_and_a_release_ends_the_quantum() {
        // Spec: launch-quantisation R21
        let (track, scene) = ids();
        let (tempo, sigs) = (tempo(), TimeSignatureMap::default());
        let mut launcher = SessionLauncher::new(16);
        launcher.set_settings(LaunchSettings {
            quantization: LaunchQuantization::Quarter,
            enabled: true,
            ..LaunchSettings::default()
        });
        let on = |at| playhead(&tempo, &sigs, true, at);
        launcher.press(track, scene, mode(LaunchMode::Repeat), on(Ticks::ZERO));
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
        launcher.release(track, scene, mode(LaunchMode::Repeat), on(release));
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
            ..LaunchSettings::default()
        });
        launcher.press(
            track,
            scene,
            mode(LaunchMode::Repeat),
            on(Ticks::QUARTER * 5),
        );
        launcher.apply_due(frame(Ticks::QUARTER * 5));
        launcher.repeat(on(Ticks::QUARTER * 5));
        assert_eq!(launcher.state_of(track, scene), Some(SlotState::Playing));
        assert_eq!(launcher.next_due(Frames::ZERO), None);
    }

    #[test]
    fn a_scene_launch_acts_as_a_trigger_press_whatever_the_mode() {
        // Spec: launch-quantisation R22
        let (mut launcher, track, scene) = playing(LaunchMode::Toggle);
        let (tempo, sigs) = (tempo(), TimeSignatureMap::default());
        let slots = [row(scene, track, LaunchMode::Toggle)];
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
    fn a_started_clip_takes_its_track_over_and_a_stop_leaves_it_so() {
        // Spec: session-view R20
        let (track, scene) = ids();
        let tempo = tempo();
        let sigs = TimeSignatureMap::default();
        let mut launcher = SessionLauncher::new(16);
        let mut playing = Vec::with_capacity(4);
        launcher.press(
            track,
            scene,
            mode(LaunchMode::Trigger),
            playhead(&tempo, &sigs, true, Ticks::QUARTER),
        );
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
                frame: Frames::ZERO,
                offset: ClipOffset::ZERO,
                // Spec: session-automation R7. The clip that stopped is
                // kept, with the frames it played between.
                last: Some(StoppedClip {
                    scene,
                    launched: bar,
                    offset: ClipOffset::ZERO,
                    stopped: tempo.ticks_to_frames(Ticks::WHOLE * 2, SampleRate::HZ_48000),
                }),
            }]
        );
        launcher.back_to_arrangement(None, Frames::ZERO);
        launcher.playback(&mut playing);
        assert!(playing.is_empty(), "Back to Arrangement forgets it");
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
        launcher.press(a, scene_a, mode(LaunchMode::Trigger), at);
        launcher.press(b, scene_b, mode(LaunchMode::Trigger), at);
        let bar = tempo.ticks_to_frames(Ticks::WHOLE, SampleRate::HZ_48000);
        launcher.apply_due(bar);
        // `a` plays with a stop queued, `b` plays, `c` has a launch queued
        // and an idle stop's mark.
        let later = playhead(&tempo, &sigs, true, Ticks::WHOLE + Ticks::QUARTER);
        launcher.stop(Some(a), &[], later);
        launcher.press(c, scene_c, mode(LaunchMode::Trigger), later);
        launcher.stop(Some(c), &[], later);
        launcher.press(c, scene_c, mode(LaunchMode::Trigger), later);
        launcher.back_to_arrangement(Some(a), Frames::ZERO);
        assert_eq!(launcher.state_of(a, scene_a), None);
        assert_eq!(launcher.overridden(), [b], "only that track returned");
        assert_eq!(launcher.state_of(b, scene_b), Some(SlotState::Playing));
        launcher.back_to_arrangement(None, Frames::ZERO);
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
        // Spec: launch-quantisation R34. Steps are whole numbers from -64 to
        // 64 other than 0.
        let nudge = |steps: i32| {
            command_from_args(
                "session.nudge",
                serde_json::json!({ "track": track, "scene": scene, "steps": steps }),
            )
        };
        for steps in [-64, -1, 1, 64] {
            assert_eq!(
                nudge(steps),
                Ok(SessionCommand::Nudge {
                    track,
                    scene,
                    steps
                })
            );
        }
        for steps in [0, 65, -65] {
            assert!(
                matches!(nudge(steps), Err(RunError::InvalidArgs { .. })),
                "{steps}"
            );
        }
        assert!(matches!(
            command_from_args(
                "session.nudge",
                serde_json::json!({ "track": track, "scene": scene, "steps": 1.5 })
            ),
            Err(RunError::InvalidArgs { .. })
        ));
        assert!(matches!(
            command_from_args("session.launch", serde_json::json!({})),
            Err(RunError::InvalidArgs { .. })
        ));
        assert_eq!(
            command_from_args("session.stop", serde_json::json!({})),
            Err(RunError::UnknownCommand("session.stop".to_owned()))
        );
    }

    /// The song frame of tick `at` at 120 BPM.
    fn at_tick(tempo: &TempoMap, at: Ticks) -> Frames {
        tempo.ticks_to_frames(at, SampleRate::HZ_48000)
    }

    fn button(mode: LaunchMode, legato: bool, quantization: Option<LaunchQuantization>) -> Button {
        Button {
            mode,
            legato,
            quantization,
        }
    }

    #[test]
    fn a_clips_own_grid_places_its_press_and_the_global_one_places_a_stop() {
        // Spec: launch-quantisation R25, R26
        let (track, scene) = ids();
        let tempo = tempo();
        let sigs = TimeSignatureMap::default();
        let mut launcher = SessionLauncher::new(16);
        // The global value is 1 Bar; the clip's own is 1/4. A press an
        // eighth past 1.1.1 starts on the next quarter, and with the global
        // switch off the clip keeps its step.
        for enabled in [true, false] {
            launcher.set_settings(LaunchSettings {
                enabled,
                ..LaunchSettings::default()
            });
            launcher.clear();
            launcher.press(
                track,
                scene,
                button(
                    LaunchMode::Trigger,
                    false,
                    Some(LaunchQuantization::Quarter),
                ),
                playhead(&tempo, &sigs, true, Ticks::QUARTER / 2),
            );
            assert_eq!(
                launcher.next_due(Frames::ZERO),
                Some(at_tick(&tempo, Ticks::QUARTER)),
                "switch {enabled}"
            );
        }
        // A stop button ignores the clip's step: global 1 Bar, so the bar line.
        launcher.set_settings(LaunchSettings::default());
        launcher.apply_due(at_tick(&tempo, Ticks::QUARTER));
        launcher.stop(
            Some(track),
            &[],
            playhead(&tempo, &sigs, true, Ticks::QUARTER + Ticks::QUARTER / 2),
        );
        assert_eq!(
            launcher.next_due(Frames::ZERO),
            Some(at_tick(&tempo, Ticks::WHOLE))
        );
    }

    #[test]
    fn a_legato_launch_takes_the_clip_time_of_the_clip_playing_and_none_when_nothing_plays() {
        // Spec: launch-quantisation R27, R28
        let (track, first) = ids();
        let (_, second) = ids();
        let tempo = tempo();
        let sigs = TimeSignatureMap::default();
        let none = Some(LaunchQuantization::None);
        let mut launcher = SessionLauncher::new(16);
        launcher.press(
            track,
            first,
            button(LaunchMode::Trigger, false, none),
            playhead(&tempo, &sigs, true, Ticks::QUARTER),
        );
        launcher.apply_due(at_tick(&tempo, Ticks::QUARTER));
        // Three quarters on, a Legato clip launches.
        let later = Ticks::QUARTER * 4;
        launcher.press(
            track,
            second,
            button(LaunchMode::Trigger, true, none),
            playhead(&tempo, &sigs, true, later),
        );
        launcher.apply_due(at_tick(&tempo, later));
        let mut status = Vec::with_capacity(4);
        launcher.status(&mut status);
        let taken = status.iter().find(|s| s.scene == second).unwrap();
        assert_eq!(taken.offset.ticks, Ticks::QUARTER * 3);
        assert_eq!(taken.offset.frames, 3 * 24_000);
        // A relaunch of the same clip keeps what it had reached.
        let again = later + Ticks::QUARTER;
        launcher.press(
            track,
            second,
            button(LaunchMode::Trigger, true, none),
            playhead(&tempo, &sigs, true, again),
        );
        launcher.apply_due(at_tick(&tempo, again));
        launcher.status(&mut status);
        assert_eq!(status[0].offset.ticks, Ticks::QUARTER * 4);
        // With the transport stopped nothing is taken over, whatever
        // "plays" in the slots: the clip starts from its start.
        let (other, third) = ids();
        launcher.press(
            other,
            third,
            button(LaunchMode::Trigger, true, none),
            playhead(&tempo, &sigs, false, again),
        );
        launcher.apply_due(at_tick(&tempo, again));
        launcher.status(&mut status);
        let started = status.iter().find(|s| s.scene == third).unwrap();
        assert_eq!(started.offset, ClipOffset::ZERO);
    }

    #[test]
    fn a_nudge_moves_a_playing_clip_by_the_global_step_and_wraps_back_into_its_loop() {
        // Spec: launch-quantisation R30 to R32
        let (track, scene) = ids();
        let tempo = tempo();
        let sigs = TimeSignatureMap::default();
        let mut launcher = SessionLauncher::new(16);
        launcher.set_settings(LaunchSettings {
            quantization: LaunchQuantization::Quarter,
            ..LaunchSettings::default()
        });
        let none = Some(LaunchQuantization::None);
        launcher.press(
            track,
            scene,
            button(LaunchMode::Trigger, false, none),
            playhead(&tempo, &sigs, true, Ticks::ZERO),
        );
        launcher.apply_due(Frames::ZERO);
        let at = Ticks::QUARTER / 2;
        let ahead = playhead(&tempo, &sigs, true, at);
        let time = |launcher: &SessionLauncher| {
            let mut status = Vec::with_capacity(4);
            launcher.status(&mut status);
            status[0].offset.ticks + at
        };
        // Forward a quarter, at once.
        launcher.nudge(track, scene, 1, None, ahead);
        assert_eq!(time(&launcher), Ticks::QUARTER * 3 / 2);
        // Back two quarters: before the start. A clip without Loop goes to
        // its start; a looping one a whole loop later.
        launcher.nudge(track, scene, -2, None, ahead);
        assert_eq!(time(&launcher), Ticks::ZERO);
        launcher.nudge(track, scene, 1, None, ahead);
        let one_bar = Some(LoopLength::Ticks(Ticks::WHOLE));
        launcher.nudge(track, scene, -2, one_bar, ahead);
        assert_eq!(
            time(&launcher),
            Ticks::QUARTER - Ticks::QUARTER * 2 + Ticks::WHOLE
        );
        // A slot that doesn't play changes nothing.
        let (_, idle) = ids();
        launcher.nudge(track, idle, 1, None, ahead);
        let mut status = Vec::with_capacity(4);
        launcher.status(&mut status);
        assert_eq!(status.len(), 1);
    }
}
