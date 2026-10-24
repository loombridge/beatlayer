//! Linked-track editing in the command layer (WP-6.23; the linked-track
//! editing spec, R7, R8, R11, R15): which lanes and clips of a link group
//! match, and the commands an edit to one track adds for the others.

use crate::clip::{Clip, ClipData};
use crate::command::Command;
use crate::document::Document;
use crate::ids::{ClipId, TakeLaneId, TrackId};

impl Document {
    /// The lane of track `other` that matches `lane` of `track`: the main
    /// lane (`Some(None)`) for the main lane, or the take lane at the same
    /// place among `other`'s take lanes; `None` when `other` has no lane
    /// there (R7, R11).
    #[must_use]
    pub fn matching_lane(
        &self,
        track: TrackId,
        lane: Option<TakeLaneId>,
        other: TrackId,
    ) -> Option<Option<TakeLaneId>> {
        let Some(lane) = lane else {
            return Some(None);
        };
        let slot = self.track(track)?.take_lanes().position(|l| l.id == lane)?;
        Some(Some(self.track(other)?.take_lanes().nth(slot)?.id))
    }

    /// The clips on a track's main lane (`None`) or on one of its take lanes.
    fn lane_clips(&self, track: TrackId, lane: Option<TakeLaneId>) -> Vec<&Clip> {
        match lane {
            None => self
                .track(track)
                .map_or_else(Vec::new, |t| t.clips().collect()),
            Some(lane) => self
                .take_lane(lane)
                .map_or_else(Vec::new, |l| l.clips.iter().collect()),
        }
    }

    /// The clips on the tracks linked with this arrangement clip's, on the
    /// matching lane, that start and end where it does (R7). Session clips
    /// have none.
    #[must_use]
    pub fn linked_clips(&self, id: ClipId) -> Vec<ClipId> {
        let (Some(clip), Some(track)) = (self.clip(id), self.clip_track(id)) else {
            return Vec::new();
        };
        let lane = self.clip_lane(id);
        if lane.is_none() && !self.track(track).is_some_and(|t| t.clips.contains_key(&id)) {
            return Vec::new();
        }
        let mut found = Vec::new();
        for other in self.linked_with(track).into_iter().filter(|t| *t != track) {
            let Some(lane) = self.matching_lane(track, lane, other) else {
                continue;
            };
            let held = self.lane_clips(other, lane);
            found.extend(
                held.into_iter()
                    .filter(|c| c.start == clip.start && c.end() == clip.end())
                    .map(|c| c.id),
            );
        }
        found
    }

    /// `command` and the commands that make the same edit on the tracks
    /// linked with the ones it names (R8, R11, R15): the edits that reach
    /// them are arming, moving and resizing a clip, a clip's fades, and
    /// renaming, deleting and auditioning take lanes. Any other command
    /// comes back alone.
    #[must_use]
    pub fn with_linked(&self, command: Command) -> Vec<Command> {
        let mut all = vec![command.clone()];
        let others = |track: TrackId| {
            self.linked_with(track)
                .into_iter()
                .filter(move |t| *t != track)
        };
        let lanes_like = |lane: TakeLaneId| -> Vec<TakeLaneId> {
            let Some(track) = self.take_lane_track(lane) else {
                return Vec::new();
            };
            others(track)
                .filter_map(|other| self.matching_lane(track, Some(lane), other)?)
                .collect()
        };
        match command {
            Command::SetArm { track, arm } => {
                all.extend(others(track).map(|track| Command::SetArm { track, arm }));
            }
            Command::ArmExclusive { track } => {
                all.extend(others(track).map(|track| Command::SetArm { track, arm: true }));
            }
            Command::RenameTakeLane { lane, name } => {
                all.extend(
                    lanes_like(lane)
                        .into_iter()
                        .map(|lane| Command::RenameTakeLane {
                            lane,
                            name: name.clone(),
                        }),
                );
            }
            Command::SetAudition { lane, on } => {
                all.extend(
                    lanes_like(lane)
                        .into_iter()
                        .map(|lane| Command::SetAudition { lane, on }),
                );
            }
            Command::DeleteTakeLanes { lanes } => {
                let mut more: Vec<TakeLaneId> = Vec::new();
                for lane in lanes.iter().flat_map(|lane| lanes_like(*lane)) {
                    if !lanes.contains(&lane) && !more.contains(&lane) {
                        more.push(lane);
                    }
                }
                if !more.is_empty() {
                    all = vec![Command::DeleteTakeLanes {
                        lanes: lanes.iter().copied().chain(more).collect(),
                    }];
                }
            }
            Command::SetClipLength { clip, length } => {
                all.extend(
                    self.linked_clips(clip)
                        .into_iter()
                        .map(|clip| Command::SetClipLength { clip, length }),
                );
            }
            Command::MoveClip { clip, track, start } => {
                let by = self.clip(clip).map(|c| start - c.start);
                if let (Some(by), true) = (by, self.clip_track(clip) == Some(track)) {
                    all.extend(self.linked_clips(clip).into_iter().filter_map(|linked| {
                        Some(Command::MoveClip {
                            clip: linked,
                            track: self.clip_track(linked)?,
                            start: self.clip(linked)?.start + by,
                        })
                    }));
                }
            }
            Command::SetClipFades {
                clip,
                fade_in,
                fade_out,
                fade_in_curve,
                fade_out_curve,
            } => all.extend(self.linked_fades(
                clip,
                (fade_in, fade_out),
                (fade_in_curve, fade_out_curve),
            )),
            _ => {}
        }
        all
    }

    /// The fade edits that match one to clip `clip`: each linked audio
    /// clip's fade-in follows where it starts with `clip`, its fade-out where
    /// it ends with it, and the other fade stays as it is (R8).
    fn linked_fades(
        &self,
        clip: ClipId,
        (fade_in, fade_out): (f64, f64),
        (fade_in_curve, fade_out_curve): (Option<f64>, Option<f64>),
    ) -> Vec<Command> {
        let Some(first) = self.clip(clip) else {
            return Vec::new();
        };
        let (Some(track), lane) = (self.clip_track(clip), self.clip_lane(clip)) else {
            return Vec::new();
        };
        let mut found: Vec<Command> = Vec::new();
        for other in self.linked_with(track).into_iter().filter(|t| *t != track) {
            let Some(lane) = self.matching_lane(track, lane, other) else {
                continue;
            };
            let held = self.lane_clips(other, lane);
            for linked in held {
                let ClipData::Audio(audio) = &linked.data else {
                    continue;
                };
                let (starts, ends) = (linked.start == first.start, linked.end() == first.end());
                if starts || ends {
                    found.push(Command::SetClipFades {
                        clip: linked.id,
                        fade_in: if starts { fade_in } else { audio.fade_in },
                        fade_out: if ends { fade_out } else { audio.fade_out },
                        fade_in_curve: if starts { fade_in_curve } else { None },
                        fade_out_curve: if ends { fade_out_curve } else { None },
                    });
                }
            }
        }
        found
    }
}
