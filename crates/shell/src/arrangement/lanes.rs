//! The Arrangement's automation lanes (WP-6.57): a track's main lane, which
//! draws its envelope over its clips, the lanes of its own under it, the
//! choosers' filters and the commands that add, remove and fold them.
//!
//! Lane 0 is the main lane and lanes 1 and up are the lanes of its own, top to
//! bottom. What they show is view state, saved with the set but no undo step.
//!
//! The behaviour is in the automation spec, R24 to R33.

use std::borrow::Cow;

use bl_core::time::Ticks;
use bl_model::{
    AutomationTarget, Document, LaneChoice, LaneViews, MixerParam, TrackId, TrackLanes,
};
use bl_shell_spec::arrangement::{
    AddAutomationLaneArgs, AutomationLane, RemoveAutomationLaneArgs, ShowAutomatedOnlyArgs,
    ShowAutomationArgs, ShowAutomationLanesArgs,
};

use super::rows::AUTOMATION_LANE_HEIGHT;
use super::{ArrangementView, ViewError, parse};
use crate::devices::DeviceView;

/// Every parameter a track's lane choosers list, in their order: the mixer's
/// controls and sends, then each device's parameters.
#[must_use]
pub fn chooser_order(
    doc: &Document,
    devices: &DeviceView,
    track: TrackId,
) -> Vec<AutomationTarget> {
    let mut order: Vec<AutomationTarget> = [
        MixerParam::Volume,
        MixerParam::Pan,
        MixerParam::Mute,
        MixerParam::SplitLeft,
        MixerParam::SplitRight,
    ]
    .into_iter()
    .map(|param| AutomationTarget::Mixer { param })
    .chain(doc.returns().map(|to| AutomationTarget::Send { to }))
    .collect();
    for device in doc
        .track(track)
        .into_iter()
        .flat_map(|track| track.devices())
    {
        if let Some(description) = devices.description(&device.kind) {
            order.extend(
                devices
                    .plugin_configured(device, description)
                    .iter()
                    .filter_map(|id| description.param(*id))
                    .filter(|param| param.automatable)
                    .map(|param| AutomationTarget::Device {
                        device: device.id,
                        param: param.id,
                    }),
            );
        }
    }
    order
}

/// The parameter after (or before) `current` in `all`, going round at the
/// ends, that `keep` accepts and that isn't `current` itself (the
/// automation editing spec, R25).
pub fn cycle_target(
    all: &[AutomationTarget],
    current: Option<AutomationTarget>,
    forward: bool,
    keep: impl Fn(&AutomationTarget) -> bool,
) -> Option<AutomationTarget> {
    let count = all.len();
    let at = current.and_then(|current| all.iter().position(|target| *target == current));
    // With nothing shown that the list holds, the search starts from just
    // outside the end it moves away from.
    let from = at.unwrap_or(if forward { count.wrapping_sub(1) } else { 0 });
    (1..=count)
        .map(|step| {
            let step = if forward { step } else { count - step };
            all[from.wrapping_add(step) % count]
        })
        .find(|target| Some(*target) != current && keep(target))
}

fn unavailable(reason: &str) -> ViewError {
    ViewError::Unavailable(reason.to_owned())
}

impl ArrangementView {
    /// What the set keeps of the lanes, to save with it.
    #[must_use]
    pub fn lane_views(&self) -> &LaneViews {
        &self.lane_views
    }

    /// Takes the lanes a set was saved with, as it opens (R33): the tracks
    /// the document has.
    pub fn restore_lanes(&mut self, views: LaneViews) {
        self.lane_views = views;
    }

    /// A track's lanes, as a new track's where it has none of its own.
    #[must_use]
    pub fn track_lanes(&self, track: TrackId) -> Cow<'_, TrackLanes> {
        self.lane_views
            .get(&track)
            .map_or_else(|| Cow::Owned(TrackLanes::default()), Cow::Borrowed)
    }

    fn store(&mut self, track: TrackId, lanes: TrackLanes) {
        if lanes == TrackLanes::default() {
            self.lane_views.remove(&track);
        } else {
            self.lane_views.insert(track, lanes);
        }
    }

    /// The parameter a track's main lane shows: the choosers' last choice,
    /// or its volume by default. `None` draws no envelope, while the lane and
    /// its choosers stay.
    #[must_use]
    pub fn automation_lane(&self, track: TrackId) -> Option<AutomationTarget> {
        self.track_lanes(track).main.target
    }

    /// What lane `lane` of a track shows and how its choosers list: 0 is the
    /// main lane, whatever the mode; the lanes of its own are there whether
    /// they show or not.
    #[must_use]
    pub fn lane_choice(&self, track: TrackId, lane: usize) -> Option<LaneChoice> {
        self.track_lanes(track).choice(lane).copied()
    }

    /// The lanes of its own a track has, top to bottom.
    #[must_use]
    pub fn own_lanes(&self, track: TrackId) -> Vec<LaneChoice> {
        self.track_lanes(track).lanes.clone()
    }

    /// Whether the header's toggle has hidden a track's lanes of its own.
    #[must_use]
    pub fn own_lanes_hidden(&self, track: TrackId) -> bool {
        self.track_lanes(track).hidden
    }

    /// How many lanes of its own a track draws: all it has while Automation
    /// Mode is on, the track is unfolded and the toggle shows them, and none
    /// otherwise.
    #[must_use]
    pub fn shown_own_lanes(&self, track: TrackId) -> usize {
        if self.automation_mode && !self.is_folded(track) {
            let lanes = self.track_lanes(track);
            if !lanes.hidden {
                return lanes.lanes.len();
            }
        }
        0
    }

    /// How tall a track's lanes of its own are together, in points. The main
    /// lane draws over the track's clips, so it adds nothing.
    #[must_use]
    pub fn automation_lane_height(&self, track: TrackId) -> f32 {
        self.shown_own_lanes(track) as f32 * AUTOMATION_LANE_HEIGHT
    }

    /// The lanes a track draws that show a parameter, and the parameter:
    /// the main lane, then the lanes of its own, while Automation Mode is on.
    #[must_use]
    pub fn shown_lane_targets(&self, track: TrackId) -> Vec<(usize, AutomationTarget)> {
        if !self.automation_mode || self.is_folded(track) {
            return Vec::new();
        }
        let lanes = self.track_lanes(track);
        let own = self.shown_own_lanes(track);
        (0..=own)
            .filter_map(|lane| Some((lane, lanes.choice(lane)?.target?)))
            .collect()
    }

    /// The number of the lane a track draws that shows `target`.
    #[must_use]
    pub fn shown_lane_of(&self, track: TrackId, target: AutomationTarget) -> Option<usize> {
        self.shown_lane_targets(track)
            .into_iter()
            .find(|(_, shown)| *shown == target)
            .map(|(lane, _)| lane)
    }

    /// The automation lane that has the keyboard: the selected lane's track
    /// and the parameter it shows, while Automation Mode is on, the track is
    /// the only one selected and unfolded, and its lane draws an envelope
    /// (the automation editing spec, R20).
    #[must_use]
    pub fn keyboard_lane(&self) -> Option<(TrackId, AutomationTarget)> {
        let (track, lane) = self.keyboard_lane_number()?;
        Some((
            track,
            self.shown_lane_targets(track)
                .into_iter()
                .find(|(n, _)| *n == lane)?
                .1,
        ))
    }

    /// The selected lane's track and number, under the same conditions as
    /// [`ArrangementView::keyboard_lane`].
    #[must_use]
    pub fn keyboard_lane_number(&self) -> Option<(TrackId, usize)> {
        let (track, lane) = self.selected_lane?;
        (self.automation_mode && self.selected == [track] && !self.is_folded(track))
            .then_some((track, lane))
    }

    /// Gives a track's lane the keyboard, or takes it from whichever has it
    /// with `None`. Taking it away also deselects the breakpoint.
    pub fn select_lane(&mut self, lane: Option<(TrackId, usize)>) {
        if self.selected_lane != lane {
            self.selected_breakpoint = None;
        }
        self.selected_lane = lane;
    }

    /// Lets go of the selected breakpoint.
    pub fn deselect_breakpoint(&mut self) {
        self.selected_breakpoint = None;
    }

    /// The breakpoint selected in the lane that has the keyboard: its track,
    /// the parameter and its position (R22).
    #[must_use]
    pub fn selected_breakpoint(&self) -> Option<(TrackId, AutomationTarget, Ticks)> {
        let (track, target) = self.keyboard_lane()?;
        self.selected_breakpoint
            .filter(|(t, g, _)| *t == track && *g == target)
    }

    /// Selects the breakpoint at `at` in a track's automation lane, or none
    /// with `None`. Selecting gives the lane the keyboard and puts the insert
    /// marker on the breakpoint.
    pub fn select_breakpoint(
        &mut self,
        track: TrackId,
        target: AutomationTarget,
        at: Option<Ticks>,
    ) {
        match at {
            Some(at) => {
                self.selected_lane = Some((track, self.shown_lane_of(track, target).unwrap_or(0)));
                self.selected = vec![track];
                self.selected_clips.clear();
                self.title_bars = false;
                self.insert_marker = at;
                self.selected_breakpoint = Some((track, target, at));
            }
            None => self.selected_breakpoint = None,
        }
    }

    /// The lane of a track a command names: its main lane if `lane` is left
    /// out or 0, else one of its own.
    fn lane_number(&self, track: TrackId, lane: Option<u32>) -> Result<usize, ViewError> {
        let lane = lane.unwrap_or(0) as usize;
        if self.track_lanes(track).choice(lane).is_none() {
            return Err(ViewError::InvalidArgs(format!(
                "track {} has no lane {lane}",
                track.0
            )));
        }
        Ok(lane)
    }

    /// `arrangement.show_automation`: the choosers pick a lane's parameter.
    pub(super) fn show_automation(
        &mut self,
        doc: &Document,
        args: serde_json::Value,
    ) -> Result<(), ViewError> {
        let ShowAutomationArgs {
            track,
            target,
            lane,
        } = parse(args)?;
        self.targets(doc, Some(vec![track]))?;
        let lane = self.lane_number(track, lane)?;
        let mut lanes = self.track_lanes(track).into_owned();
        if let Some(choice) = lanes.choice_mut(lane) {
            choice.target = target;
        }
        self.store(track, lanes);
        // The breakpoint selected in the lane belongs to what it showed.
        if self
            .selected_breakpoint
            .is_some_and(|(t, shown, _)| t == track && Some(shown) != target)
        {
            self.selected_breakpoint = None;
        }
        Ok(())
    }

    /// `arrangement.add_automation_lane`: an envelope moves into a lane of
    /// its own (R25), or the lane it already has is selected. `order` is the
    /// choosers' order for the track.
    pub fn add_lane(
        &mut self,
        doc: &Document,
        order: &[AutomationTarget],
        args: AddAutomationLaneArgs,
    ) -> Result<(), ViewError> {
        let AddAutomationLaneArgs {
            track,
            target,
            lane,
            all,
        } = args;
        self.targets(doc, Some(vec![track]))?;
        let Some(owner) = doc.track(track) else {
            return Ok(());
        };
        let has_envelope = |target: &AutomationTarget| owner.automation_for(target).is_some();
        let mut lanes = self.track_lanes(track).into_owned();
        if all {
            // Every automated parameter with no lane of its own, in the
            // choosers' order; one the choosers don't list goes last.
            let mut automated: Vec<AutomationTarget> = owner
                .automation()
                .map(|(target, _)| *target)
                .filter(|target| !lanes.lanes.iter().any(|l| l.target == Some(*target)))
                .collect();
            automated.sort_by_key(|target| {
                (
                    order.iter().position(|t| t == target).unwrap_or(usize::MAX),
                    *target,
                )
            });
            lanes
                .lanes
                .extend(automated.into_iter().map(|t| LaneChoice::showing(Some(t))));
            let main_moved = lanes
                .main
                .target
                .is_some_and(|main| lanes.lanes.iter().any(|l| l.target == Some(main)));
            if main_moved {
                let next = self.next_unlaned(order, &lanes, 0, lanes.main.target, has_envelope);
                lanes.main.target = next;
            }
        } else {
            // The lane whose envelope moves, if it is a lane that shows one.
            let source = match target {
                Some(target) => (lanes.main.target == Some(target)).then_some(0),
                None => Some(self.lane_number(track, lane)?),
            };
            let Some(moving) = target.or_else(|| {
                source.and_then(|source| lanes.choice(source).and_then(|choice| choice.target))
            }) else {
                return Err(unavailable(
                    "the lane shows None, so there is nothing to move",
                ));
            };
            let existing = lanes
                .lanes
                .iter()
                .enumerate()
                .find(|(index, l)| Some(index + 1) != source && l.target == Some(moving))
                .map(|(index, _)| index + 1);
            if let Some(existing) = existing {
                if target.is_none() {
                    return Err(unavailable("the parameter already has a lane"));
                }
                // Spec: automation R29. The lane the parameter has is selected.
                self.select_lane(Some((track, existing)));
                self.selected = vec![track];
                self.title_bars = false;
                return Ok(());
            }
            lanes.lanes.push(LaneChoice::showing(Some(moving)));
            if let Some(source) = source {
                let next = self.next_unlaned(order, &lanes, source, Some(moving), has_envelope);
                if let Some(choice) = lanes.choice_mut(source) {
                    choice.target = next;
                }
            }
        }
        lanes.hidden = false;
        self.store(track, lanes);
        self.clamp_scroll(doc);
        Ok(())
    }

    /// The next parameter after `after` in the choosers' order that has an
    /// envelope and no lane but `lane`'s own: what a lane shows once its
    /// envelope has moved out, or `None` (R25).
    fn next_unlaned(
        &self,
        order: &[AutomationTarget],
        lanes: &TrackLanes,
        lane: usize,
        after: Option<AutomationTarget>,
        has_envelope: impl Fn(&AutomationTarget) -> bool,
    ) -> Option<AutomationTarget> {
        cycle_target(order, after, true, |target| {
            has_envelope(target)
                && (0..=lanes.lanes.len())
                    .filter(|n| *n != lane)
                    .all(|n| lanes.choice(n).and_then(|c| c.target) != Some(*target))
        })
    }

    /// `arrangement.remove_automation_lane`: a lane of its own leaves the
    /// view (R26).
    pub(super) fn remove_lane(
        &mut self,
        doc: &Document,
        args: serde_json::Value,
    ) -> Result<(), ViewError> {
        let RemoveAutomationLaneArgs {
            track,
            lane,
            and_after,
        } = parse(args)?;
        self.targets(doc, Some(vec![track]))?;
        let lane = lane as usize;
        let mut lanes = self.track_lanes(track).into_owned();
        if lane == 0 || lane > lanes.lanes.len() {
            return Err(ViewError::InvalidArgs(format!(
                "track {} has no lane {lane} of its own",
                track.0
            )));
        }
        let end = if and_after { lanes.lanes.len() } else { lane };
        lanes.lanes.drain(lane - 1..end);
        // The selected lane moves up with the lanes after the ones removed,
        // or goes with them.
        if let Some((selected, n)) = self.selected_lane
            && selected == track
            && n >= lane
        {
            self.selected_lane = (n > end).then_some((track, n - (end - lane + 1)));
            self.selected_breakpoint = None;
        }
        self.store(track, lanes);
        self.clamp_scroll(doc);
        Ok(())
    }

    /// `arrangement.show_automation_lanes`: the header's toggle (R26).
    pub(super) fn show_lanes(
        &mut self,
        doc: &Document,
        args: serde_json::Value,
    ) -> Result<(), ViewError> {
        let ShowAutomationLanesArgs { tracks, on } = parse(args)?;
        let tracks = self.targets(doc, tracks)?;
        let Some(first) = tracks.first() else {
            return Err(unavailable("no track is selected"));
        };
        let on = on.unwrap_or(self.own_lanes_hidden(*first));
        for track in tracks {
            let mut lanes = self.track_lanes(track).into_owned();
            lanes.hidden = !on;
            self.store(track, lanes);
        }
        self.clamp_scroll(doc);
        Ok(())
    }

    /// `arrangement.show_automated_only`: a lane's filter (R27).
    pub(super) fn show_automated_only(
        &mut self,
        doc: &Document,
        args: serde_json::Value,
    ) -> Result<(), ViewError> {
        let ShowAutomatedOnlyArgs { track, lane, on } = parse(args)?;
        self.targets(doc, Some(vec![track]))?;
        let lane = self.lane_number(track, lane)?;
        let mut lanes = self.track_lanes(track).into_owned();
        if let Some(choice) = lanes.choice_mut(lane) {
            choice.only_automated = on.unwrap_or(!choice.only_automated);
        }
        self.store(track, lanes);
        Ok(())
    }

    /// The lanes as `arrangement.state` reports them.
    pub(super) fn lane_state(&self) -> Vec<AutomationLane> {
        self.lane_views
            .iter()
            .map(|(track, lanes)| AutomationLane {
                track: *track,
                target: lanes.main.target,
                only_automated: lanes.main.only_automated,
                lanes: lanes.lanes.clone(),
                lanes_hidden: lanes.hidden,
            })
            .collect()
    }
}
