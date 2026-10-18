//! Follow actions in the engine (WP-3.10): what a Session clip or a scene
//! does by itself once it has played for its follow time.
//!
//! The program carries a [`FollowTable`]: each slot clip's follow actions,
//! its own launch quantisation and how long its loop (or, with the loop off,
//! its play) lasts, and each scene's follow actions, in the scenes' order.
//! The launcher arms a follow when a clip starts or a scene launches (the
//! follow time counted from its launch tick), the processor splits its block
//! at the follow frame as it does for a launch, and [`SessionLauncher::run_follow`]
//! then picks side A or B, resolves the action over the clip's group and
//! launches, stops or restarts, with no allocation.
//!
//! The draw behind A, B, Any and Other is a pure function of the clip's (or
//! scene's) ID and the song frame it started on, so two renders of one set
//! make the same choices, bit for bit, whatever the block size.

use bl_core::time::{Frames, Ticks};
use bl_model::{
    ClipData, Document, FOLLOW_TIME_MIN, FollowAction, FollowActions, LaunchMode,
    LaunchQuantization, SceneId, TrackId,
};

use super::{Playhead, SceneRun, SessionLauncher};
use crate::program::EngineProgram;

/// How many follow actions one call fires at most, so a set whose actions
/// feed each other on one frame can't spin.
const MAX_FIRES: usize = 64;

/// Where a slot's or a scene's follow is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FollowState {
    /// The clip started and its follow isn't worked out yet.
    Unarmed,
    /// The follow runs at `due`, the frame of the launch tick plus the
    /// follow time.
    Armed {
        /// The tick it runs at, which a launched clip counts its start from.
        tick: Ticks,
        /// The song frame of `tick`.
        due: Frames,
    },
    /// Nothing more runs until the clip starts again.
    Spent,
}

impl FollowState {
    /// The frame the follow runs at, while it's armed.
    pub(super) fn due(self) -> Option<Frames> {
        match self {
            Self::Armed { due, .. } => Some(due),
            _ => None,
        }
    }
}

/// What the program knows of a slot clip for its follow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SlotInfo {
    track: TrackId,
    scene: SceneId,
    quantization: Option<LaunchQuantization>,
    follow: FollowActions,
    seed: u64,
    /// The length Linked counts: the loop brace's, or with the loop off,
    /// how long the clip plays.
    length: Ticks,
    looping: bool,
}

/// What the program knows of a scene for its follow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SceneInfo {
    scene: SceneId,
    follow: FollowActions,
    seed: u64,
}

/// Every slot clip's and scene's follow data, built with the program.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct FollowTable {
    /// The scenes in the set's order.
    scenes: Vec<SceneInfo>,
    /// The slot clips, sorted by track and then scene.
    slots: Vec<SlotInfo>,
}

impl FollowTable {
    /// The table for `document`.
    pub(crate) fn of(document: &Document) -> Self {
        let scenes = document
            .scenes()
            .map(|scene| SceneInfo {
                scene: scene.id,
                follow: scene.follow,
                seed: mix(scene.id.0.get()),
            })
            .collect();
        let mut slots = Vec::new();
        for track in document.track_order() {
            let Some(track) = document.track(track) else {
                continue;
            };
            for (scene, clip) in track.slots() {
                let (length, looping) = play_length(clip.length, &clip.data);
                slots.push(SlotInfo {
                    track: track.id(),
                    scene,
                    quantization: clip.launch.quantization,
                    follow: clip.launch.follow,
                    seed: mix(clip.id.0.get()),
                    length,
                    looping,
                });
            }
        }
        slots.sort_unstable_by_key(|info| (info.track, info.scene));
        Self { scenes, slots }
    }

    fn slot(&self, track: TrackId, scene: SceneId) -> Option<&SlotInfo> {
        self.slots
            .binary_search_by_key(&(track, scene), |info| (info.track, info.scene))
            .ok()
            .map(|at| &self.slots[at])
    }

    fn scene(&self, scene: SceneId) -> Option<(usize, &SceneInfo)> {
        self.scenes
            .iter()
            .enumerate()
            .find(|(_, info)| info.scene == scene)
    }

    fn scene_holds_a_clip(&self, scene: SceneId) -> bool {
        self.slots.iter().any(|info| info.scene == scene)
    }

    /// How long a scene's follow waits after its launch: the multiplier
    /// times its longest clip when Longest is on, else its own time. `None`
    /// for Longest on a scene with no clip.
    fn scene_ticks(&self, info: &SceneInfo) -> Option<Ticks> {
        if !info.follow.linked {
            return Some(follow_ticks(&info.follow, Ticks::ZERO, false));
        }
        self.slots
            .iter()
            .filter(|slot| slot.scene == info.scene)
            .map(|slot| follow_ticks(&info.follow, slot.length, slot.looping))
            .max()
    }
}

/// How long a clip plays per pass, and whether it loops: the loop brace
/// while its Loop switch is on, else from the Start marker to the End marker
/// or the clip's end. `shortcut:` an audio clip's brace is in seconds, so it
/// counts the clip's own length; convert it when warping brings audio
/// loops in ticks.
fn play_length(length: Ticks, data: &ClipData) -> (Ticks, bool) {
    match data {
        ClipData::Midi(midi) if midi.looping => {
            let end = midi.loop_end.unwrap_or_else(|| {
                (midi.loop_start + length).max(midi.loop_start + Ticks::QUARTER)
            });
            (end - midi.loop_start, true)
        }
        ClipData::Midi(midi) => {
            let end = midi.end.unwrap_or(midi.start + length);
            (length.min(end - midi.start), false)
        }
        ClipData::Audio(audio) => (length, audio.looping),
    }
}

/// The follow time in ticks: the multiplier times the loop while the loop
/// is on, the clip's end while it is off, or the Unlinked time; never under
/// the shortest.
fn follow_ticks(follow: &FollowActions, length: Ticks, looping: bool) -> Ticks {
    let ticks = match (follow.linked, looping) {
        (false, _) => follow.time,
        (true, true) => length * i64::from(follow.multiplier),
        (true, false) => length,
    };
    ticks.max(FOLLOW_TIME_MIN)
}

/// A well-mixed 64-bit value from `x` (SplitMix64's finaliser).
fn mix(x: u64) -> u64 {
    let x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    let x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// The draw for a clip or scene with `seed` that started on `started`.
pub(crate) fn draw(seed: u64, started: Frames) -> u64 {
    mix(seed ^ mix(started.get() as u64))
}

/// Side A or side B for `draw`: A with the probability `chance_a` / 100.
pub(crate) fn pick(follow: &FollowActions, draw: u64) -> (FollowAction, u32) {
    if draw % 100 < u64::from(follow.chance_a) {
        (follow.action_a, follow.jump_a)
    } else {
        (follow.action_b, follow.jump_b)
    }
}

/// What a follow action comes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Nothing happens.
    Nothing,
    /// The clip (or scene) stops.
    Stop,
    /// The clip (or scene) at this index launches.
    Launch(usize),
}

/// What `action` does for the unit at `here` of `count`, in its group: the
/// run of units around it for which `occupied` is true (an empty slot, or a
/// scene with no clip, splits groups). `jump` is the 1-based number Jump
/// goes to; `draw` chooses for Any and Other.
pub(crate) fn resolve(
    action: FollowAction,
    jump: u32,
    here: usize,
    count: usize,
    occupied: impl Fn(usize) -> bool,
    draw: u64,
) -> Outcome {
    let (mut first, mut last) = (here, here);
    while first > 0 && occupied(first - 1) {
        first -= 1;
    }
    while last + 1 < count && occupied(last + 1) {
        last += 1;
    }
    match action {
        FollowAction::NoAction => Outcome::Nothing,
        FollowAction::Stop => Outcome::Stop,
        FollowAction::PlayAgain => Outcome::Launch(here),
        FollowAction::Previous => Outcome::Launch(if here > first { here - 1 } else { last }),
        FollowAction::Next => Outcome::Launch(if here < last { here + 1 } else { first }),
        FollowAction::First => Outcome::Launch(first),
        FollowAction::Last => Outcome::Launch(last),
        FollowAction::Any => Outcome::Launch(first + (draw % (last - first + 1) as u64) as usize),
        FollowAction::Other if first == last => Outcome::Launch(here),
        FollowAction::Other => {
            let pick = first + (draw % (last - first) as u64) as usize;
            Outcome::Launch(if pick >= here { pick + 1 } else { pick })
        }
        FollowAction::Jump => match (jump as usize).checked_sub(1) {
            Some(target) if target < count && occupied(target) => Outcome::Launch(target),
            _ => Outcome::Stop,
        },
    }
}

/// The tick and frame a follow launch starts the target clip on: the follow
/// tick, or the first line of the clip's own quantisation at or after it. The
/// global quantisation is ignored (observed in Live 12.4.3).
fn start_of(
    quantization: Option<LaunchQuantization>,
    tick: Ticks,
    playhead: Playhead<'_>,
) -> (Ticks, Frames) {
    let line = quantization.map_or(tick, |step| {
        // The first line after the tick before, so a follow that lands on
        // the grid starts there.
        step.next_line(tick - Ticks(1), playhead.signatures)
    });
    (line, playhead.frames(line))
}

impl SessionLauncher {
    /// Arms the follows of clips that started and of a launched scene,
    /// fires those that are due at or before `at`, and applies what they
    /// queued on the same frame. The processor calls it after
    /// [`Self::apply_due`] at the start of every block part.
    pub(crate) fn run_follow(
        &mut self,
        program: &EngineProgram,
        playhead: Playhead<'_>,
        at: Frames,
    ) {
        self.arm_follow(program, playhead);
        for _ in 0..MAX_FIRES {
            if !self.fire_one(program, playhead, at) {
                break;
            }
            self.apply_due(at);
            self.arm_follow(program, playhead);
        }
    }

    /// Starts a scene's follow when it launches, counted from `line`; a
    /// scene launched later replaces it.
    pub(crate) fn start_scene_run(&mut self, scene: SceneId, line: Ticks) {
        self.scene_run = Some(SceneRun {
            scene,
            line,
            follow: FollowState::Unarmed,
        });
    }

    fn arm_follow(&mut self, program: &EngineProgram, playhead: Playhead<'_>) {
        let global = self.settings.follow_actions;
        let table = &program.follow;
        for slot in &mut self.slots {
            if slot.started.is_none() || slot.follow != FollowState::Unarmed {
                continue;
            }
            slot.follow = match table.slot(slot.track, slot.scene) {
                Some(info) if global && info.follow.enabled => {
                    let tick = slot.line + follow_ticks(&info.follow, info.length, info.looping);
                    FollowState::Armed {
                        tick,
                        due: playhead.frames(tick),
                    }
                }
                _ => FollowState::Spent,
            };
        }
        if let Some(run) = &mut self.scene_run
            && run.follow == FollowState::Unarmed
        {
            run.follow = match table.scene(run.scene) {
                Some((_, info)) if global && info.follow.enabled => match table.scene_ticks(info) {
                    Some(ticks) => {
                        let tick = run.line + ticks;
                        FollowState::Armed {
                            tick,
                            due: playhead.frames(tick),
                        }
                    }
                    None => FollowState::Spent,
                },
                _ => FollowState::Spent,
            };
        }
    }

    /// Runs the oldest follow due at or before `at`; whether one was due.
    fn fire_one(&mut self, program: &EngineProgram, playhead: Playhead<'_>, at: Frames) -> bool {
        let slot = self
            .slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| Some((slot.follow.due().filter(|&due| due <= at)?, index)))
            .min();
        let scene = self
            .scene_run
            .and_then(|run| run.follow.due().filter(|&due| due <= at));
        match (slot, scene) {
            (Some((due, index)), scene) if scene.is_none_or(|scene| due <= scene) => {
                self.fire_slot(index, program, playhead);
            }
            (_, Some(_)) => self.fire_scene(program, playhead),
            _ => return false,
        }
        true
    }

    fn fire_slot(&mut self, index: usize, program: &EngineProgram, playhead: Playhead<'_>) {
        let slot = self.slots[index];
        let FollowState::Armed { tick, due } = slot.follow else {
            return;
        };
        self.slots[index].follow = FollowState::Spent;
        let table = &program.follow;
        let (Some(info), Some((here, _))) =
            (table.slot(slot.track, slot.scene), table.scene(slot.scene))
        else {
            return;
        };
        // A stop queued by hand ends the clip; the follow leaves it be.
        if !self.settings.follow_actions || (slot.due.is_some() && !slot.restart) {
            return;
        }
        let draw = draw(info.seed, slot.started.unwrap_or(due));
        let (action, jump) = pick(&info.follow, draw);
        let occupied = |at: usize| table.slot(slot.track, table.scenes[at].scene).is_some();
        match resolve(action, jump, here, table.scenes.len(), occupied, mix(draw)) {
            Outcome::Nothing => {}
            Outcome::Stop => self.slots[index].stop_at(due),
            Outcome::Launch(target) => {
                let scene = table.scenes[target].scene;
                let quantization = table.slot(slot.track, scene).and_then(|i| i.quantization);
                let (line, at) = start_of(quantization, tick, playhead);
                self.press_at(slot.track, scene, LaunchMode::Trigger, line, at);
                // A target waiting for its own grid leaves the track silent
                // from the follow time (observed in Live 12.4.3).
                if scene != slot.scene
                    && let Some(old) = self.find_mut(slot.track, slot.scene)
                {
                    old.stop_at(due);
                }
            }
        }
    }

    fn fire_scene(&mut self, program: &EngineProgram, playhead: Playhead<'_>) {
        let Some(run) = self.scene_run else {
            return;
        };
        let FollowState::Armed { tick, due } = run.follow else {
            return;
        };
        self.scene_run = None;
        let table = &program.follow;
        let Some((here, info)) = table.scene(run.scene) else {
            return;
        };
        if !self.settings.follow_actions {
            return;
        }
        let draw = draw(info.seed, playhead.frames(run.line));
        let (action, jump) = pick(&info.follow, draw);
        let occupied = |at: usize| table.scene_holds_a_clip(table.scenes[at].scene);
        match resolve(action, jump, here, table.scenes.len(), occupied, mix(draw)) {
            Outcome::Nothing => {
                self.scene_run = Some(SceneRun {
                    follow: FollowState::Spent,
                    ..run
                })
            }
            Outcome::Stop => {
                for slot in &mut self.slots {
                    if slot.scene == run.scene && slot.started.is_some() {
                        slot.stop_at(due);
                    }
                }
            }
            Outcome::Launch(target) => {
                let scene = table.scenes[target].scene;
                for &(_, track, _) in program.scene_slots(scene) {
                    let quantization = table.slot(track, scene).and_then(|i| i.quantization);
                    let (line, at) = start_of(quantization, tick, playhead);
                    self.press_at(track, scene, LaunchMode::Trigger, line, at);
                }
                self.start_scene_run(scene, tick);
                if program.scene_maps.iter().any(|maps| maps.scene == scene) {
                    self.queue_scene(scene, playhead.frames(tick));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(_: usize) -> bool {
        true
    }

    #[test]
    fn next_and_previous_wrap_inside_the_group() {
        // Spec: follow-actions R5. Slots 1 to 3 hold clips, 4 is empty, 5 is
        // alone (observed in Live 12.4.3).
        let held = |at: usize| at != 3;
        let go = |action, here| resolve(action, 1, here, 8, held, 0);
        assert_eq!(go(FollowAction::Next, 0), Outcome::Launch(1));
        assert_eq!(go(FollowAction::Next, 2), Outcome::Launch(0));
        assert_eq!(go(FollowAction::Previous, 0), Outcome::Launch(2));
        assert_eq!(go(FollowAction::Previous, 4), Outcome::Launch(7));
        let held = |at: usize| at < 3 || at == 4;
        let go = |action, here| resolve(action, 1, here, 8, held, 0);
        assert_eq!(go(FollowAction::Next, 2), Outcome::Launch(0));
        assert_eq!(go(FollowAction::Next, 4), Outcome::Launch(4), "alone");
        assert_eq!(go(FollowAction::Previous, 4), Outcome::Launch(4));
        assert_eq!(go(FollowAction::First, 1), Outcome::Launch(0));
        assert_eq!(go(FollowAction::Last, 1), Outcome::Launch(2));
        assert_eq!(go(FollowAction::First, 4), Outcome::Launch(4));
    }

    #[test]
    fn any_may_pick_the_clip_and_other_never_does_unless_alone() {
        // Spec: follow-actions R5
        let mut any = [0; 4];
        let mut other = [0; 4];
        for n in 0..4000_u64 {
            let d = mix(n);
            let Outcome::Launch(a) = resolve(FollowAction::Any, 1, 1, 4, all, d) else {
                panic!("Any launches");
            };
            any[a] += 1;
            let Outcome::Launch(o) = resolve(FollowAction::Other, 1, 1, 4, all, d) else {
                panic!("Other launches");
            };
            other[o] += 1;
        }
        assert!(
            any.iter().all(|&n| n > 800),
            "Any reaches every clip: {any:?}"
        );
        assert_eq!(other[1], 0, "Other never picks the clip");
        assert!(
            other[0] > 1000 && other[2] > 1000 && other[3] > 1000,
            "{other:?}"
        );
        let alone = |at: usize| at == 2;
        assert_eq!(
            resolve(FollowAction::Other, 1, 2, 4, alone, 7),
            Outcome::Launch(2)
        );
    }

    #[test]
    fn jump_goes_to_its_slot_and_an_empty_target_stops() {
        // Spec: follow-actions R3
        let held = |at: usize| at != 3;
        let jump = |number| resolve(FollowAction::Jump, number, 0, 8, held, 0);
        assert_eq!(jump(3), Outcome::Launch(2));
        assert_eq!(jump(4), Outcome::Stop, "an empty slot");
        assert_eq!(jump(0), Outcome::Stop, "the field is empty");
        assert_eq!(jump(9), Outcome::Stop, "past the scenes");
        assert_eq!(resolve(FollowAction::Stop, 1, 0, 8, held, 0), Outcome::Stop);
        assert_eq!(
            resolve(FollowAction::NoAction, 1, 0, 8, held, 0),
            Outcome::Nothing
        );
        assert_eq!(
            resolve(FollowAction::PlayAgain, 1, 5, 8, held, 0),
            Outcome::Launch(5)
        );
    }

    #[test]
    fn the_a_b_split_matches_the_chance_and_the_draw_repeats() {
        // Spec: follow-actions R4, R12
        let follow = FollowActions {
            action_a: FollowAction::First,
            action_b: FollowAction::Last,
            chance_a: 33,
            ..FollowActions::default()
        };
        let seed = mix(42);
        let a = (0..30_000_i64)
            .filter(|&frame| {
                pick(&follow, draw(seed, Frames(frame * 48_000))).0 == FollowAction::First
            })
            .count();
        assert!((a as f64 / 30_000.0 - 0.33).abs() < 0.01, "{a} of 30000");
        assert_eq!(draw(seed, Frames(96_000)), draw(seed, Frames(96_000)));
        assert_ne!(draw(seed, Frames(96_000)), draw(mix(43), Frames(96_000)));
        let always_a = FollowActions {
            chance_a: 100,
            ..follow
        };
        let never_a = FollowActions {
            chance_a: 0,
            ..follow
        };
        for n in 0..1000 {
            assert_eq!(pick(&always_a, mix(n)).0, FollowAction::First);
            assert_eq!(pick(&never_a, mix(n)).0, FollowAction::Last);
        }
    }

    #[test]
    fn follow_times_count_loops_or_the_clip_end_or_the_unlinked_time() {
        // Spec: follow-actions R9
        let bar = Ticks::WHOLE;
        let linked = FollowActions {
            multiplier: 3,
            ..FollowActions::default()
        };
        assert_eq!(follow_ticks(&linked, bar, true), bar * 3);
        assert_eq!(follow_ticks(&linked, bar, false), bar, "Clip End");
        let unlinked = FollowActions {
            linked: false,
            time: bar * 2,
            ..FollowActions::default()
        };
        assert_eq!(follow_ticks(&unlinked, bar, true), bar * 2);
        assert_eq!(follow_ticks(&linked, Ticks::ZERO, true), FOLLOW_TIME_MIN);
    }
}
