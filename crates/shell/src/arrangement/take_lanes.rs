//! Take lanes in the Arrangement's view (WP-6.22; the comping spec, R23 to
//! R28): which tracks show theirs, how tall each is, which headers are
//! selected, and the lanes a selection is on. All of it is view state: none
//! of it is an undo step, and the set never changes here.

use bl_model::{Document, TakeLaneId, TrackId, TrackKind};
use bl_shell_spec::arrangement::{self as ids, Lane, SelectTakeLanesArgs, ShowTakeLanesArgs};

use super::rows::{DEFAULT_TAKE_LANE_HEIGHT, MAX_TRACK_HEIGHT, MIN_TRACK_HEIGHT, Row, TakeLaneRow};
use super::{ArrangementView, ViewError, parse};

/// Whether `track` can have take lanes: audio and MIDI tracks do.
fn holds_takes(doc: &Document, track: TrackId) -> bool {
    doc.track(track)
        .is_some_and(|track| matches!(track.kind(), TrackKind::Audio | TrackKind::Midi))
}

impl ArrangementView {
    /// How tall a take lane is.
    #[must_use]
    pub fn take_lane_height(&self, lane: TakeLaneId) -> f32 {
        self.take_heights
            .get(&lane)
            .copied()
            .unwrap_or(DEFAULT_TAKE_LANE_HEIGHT)
    }

    /// Whether the switch that shows `track`'s take lanes is on (R24): a
    /// track with no take lane, a folded one, or any track while Automation
    /// Mode is on shows none whatever the switch says.
    #[must_use]
    pub fn shows_take_lanes(&self, track: TrackId) -> bool {
        self.shown_take_lanes.contains(&track)
    }

    /// Whether the Show/Hide Take Lanes toggle shows on `track`'s header: it
    /// has a take lane, it's unfolded, and Automation Mode is off (R24).
    #[must_use]
    pub fn take_toggle_shown(&self, doc: &Document, track: TrackId) -> bool {
        !self.automation_mode
            && !self.is_folded(track)
            && doc
                .track(track)
                .is_some_and(|track| track.take_lanes().len() > 0)
    }

    /// How tall `track`'s take lanes are together, in points: 0 unless they
    /// show (R23, R24).
    #[must_use]
    pub fn take_lanes_height(&self, doc: &Document, track: TrackId) -> f32 {
        if !self.shows_take_lanes(track) || !self.take_toggle_shown(doc, track) {
            return 0.0;
        }
        doc.track(track).map_or(0.0, |track| {
            track
                .take_lanes()
                .map(|lane| self.take_lane_height(lane.id))
                .sum()
        })
    }

    /// The rows of `row`'s take lanes, nearest the main lane first, under its
    /// lane and automation lane (R23). None while they're hidden.
    pub fn take_lane_rows<'a>(
        &'a self,
        doc: &'a Document,
        row: &Row,
    ) -> impl Iterator<Item = TakeLaneRow> + 'a {
        let mut top = row.top + row.height + row.automation;
        let lanes = (row.take > 0.0)
            .then(|| doc.track(row.track))
            .flatten()
            .into_iter()
            .flat_map(bl_model::Track::take_lanes);
        lanes.map(move |lane| {
            let height = self.take_lane_height(lane.id);
            let out = TakeLaneRow {
                lane: lane.id,
                top,
                height,
            };
            top += height;
            out
        })
    }

    /// The take lanes whose headers are selected, in the order they were
    /// selected (R26).
    #[must_use]
    pub fn selected_take_lanes(&self) -> &[TakeLaneId] {
        &self.selected_take_lanes
    }

    /// Whether a take lane's header is selected.
    #[must_use]
    pub fn is_take_lane_selected(&self, lane: TakeLaneId) -> bool {
        self.selected_take_lanes.contains(&lane)
    }

    /// The take lanes the selection is on, in the order they were selected:
    /// the headers selected, otherwise the take lanes the time selection or
    /// the selected clips cover on the selected tracks (R15, R17).
    #[must_use]
    pub fn selection_take_lanes(&self) -> Vec<TakeLaneId> {
        if !self.selected_take_lanes.is_empty() {
            return self.selected_take_lanes.clone();
        }
        self.time_selection_lanes
            .iter()
            .filter_map(|lane| match lane {
                Lane::Take { track, lane } if self.selected.contains(track) => Some(*lane),
                _ => None,
            })
            .collect()
    }

    /// Whether the selection is on a take lane: the key bindings of the
    /// `take-lanes` context apply (R28).
    #[must_use]
    pub fn take_lane_selected(&self) -> bool {
        !self.selection_take_lanes().is_empty()
    }

    /// Selects take lanes' headers, as clicking them does (R26). Their tracks
    /// become the selected tracks, and the clips and time are let go.
    pub(crate) fn select_take_lane_headers(&mut self, doc: &Document, lanes: Vec<TakeLaneId>) {
        let mut tracks: Vec<TrackId> = Vec::new();
        for track in lanes.iter().filter_map(|lane| doc.take_lane_track(*lane)) {
            if !tracks.contains(&track) {
                tracks.push(track);
            }
        }
        if !lanes.is_empty() {
            self.selected = tracks;
            self.title_bars = false;
            self.selected_clips.clear();
            self.selected_fade = None;
            self.time_selection = None;
            self.time_selection_lanes.clear();
        }
        self.selected_take_lanes = lanes;
    }

    /// Selects the clips of the take lanes whose headers are selected, as
    /// selecting a header does with the Clip View or the Device View open
    /// (R26), so the Clip View shows the lane's clip. The headers stay
    /// selected, and Delete and Cmd+D still act on the lanes.
    pub(crate) fn select_header_clips(&mut self, doc: &Document) {
        let clips = self
            .selected_take_lanes
            .iter()
            .filter_map(|lane| doc.take_lane(*lane))
            .flat_map(|lane| lane.clips.iter().map(|clip| clip.id))
            .collect();
        self.selected_clips = clips;
    }

    /// Drops what the document no longer has: selected, sized and shown take
    /// lanes.
    pub(crate) fn forget_missing_take_lanes(&mut self, doc: &Document) {
        self.selected_take_lanes
            .retain(|lane| doc.take_lane(*lane).is_some());
        self.take_heights
            .retain(|lane, _| doc.take_lane(*lane).is_some());
        self.shown_take_lanes
            .retain(|track| doc.track(*track).is_some());
        self.time_selection_lanes.retain(|lane| match lane {
            Lane::Take { lane, .. } => doc.take_lane(*lane).is_some(),
            _ => true,
        });
    }

    /// Shows `track`'s take lanes, leaving Automation Mode to do it (R24).
    /// Show Take Lanes and Insert Take Lane run it.
    pub fn show_take_lanes_of(&mut self, track: TrackId) {
        self.shown_take_lanes.insert(track);
        self.automation_mode = false;
    }

    /// Marks `track`'s take lanes as shown, as a recording that added one
    /// does (R13), leaving Automation Mode as it is: they show once it's off.
    pub fn keep_take_lanes_shown(&mut self, track: TrackId) {
        self.shown_take_lanes.insert(track);
    }

    /// Sets the height of take lanes, clamped as tracks' are (R28).
    pub(crate) fn set_take_lane_heights(
        &mut self,
        doc: &Document,
        lanes: &[TakeLaneId],
        height: impl Fn(f32) -> Result<f32, ViewError>,
    ) -> Result<(), ViewError> {
        for lane in lanes {
            if doc.take_lane(*lane).is_none() {
                return Err(ViewError::InvalidArgs(format!(
                    "no take lane has the ID {}",
                    lane.0
                )));
            }
        }
        for lane in lanes {
            let new = height(self.take_lane_height(*lane))?;
            self.take_heights
                .insert(*lane, new.clamp(MIN_TRACK_HEIGHT, MAX_TRACK_HEIGHT));
        }
        Ok(())
    }

    /// Whether a person can run take-lane view command `id` with `args`
    /// now (R24, R29): there's an audio or MIDI track to show the take lanes
    /// of, or a take lane to select.
    #[must_use]
    pub(crate) fn can_run_take_lanes(
        &self,
        id: &str,
        args: &serde_json::Value,
        doc: &Document,
    ) -> bool {
        match id {
            ids::SHOW_TAKE_LANES => {
                let tracks: Vec<TrackId> = args
                    .get("tracks")
                    .and_then(|tracks| serde_json::from_value(tracks.clone()).ok())
                    .unwrap_or_else(|| self.selected.clone());
                let title_bars = args.get("title_bars").and_then(serde_json::Value::as_bool);
                // Left, from a take lane's header, needs one to be selected.
                if title_bars == Some(true) && self.selected_take_lanes.is_empty() {
                    return false;
                }
                tracks.iter().any(|track| holds_takes(doc, *track))
            }
            ids::SELECT_TAKE_LANES => true,
            _ => false,
        }
    }

    /// Runs `arrangement.select_take_lanes` and `arrangement.show_take_lanes`.
    pub(crate) fn run_take_lanes(
        &mut self,
        id: &str,
        args: serde_json::Value,
        doc: &Document,
    ) -> Result<(), ViewError> {
        match id {
            ids::SELECT_TAKE_LANES => {
                let SelectTakeLanesArgs { lanes, add, toggle } = parse(args)?;
                if let Some(missing) = lanes.iter().find(|lane| doc.take_lane(**lane).is_none()) {
                    return Err(ViewError::InvalidArgs(format!(
                        "no take lane has the ID {}",
                        missing.0
                    )));
                }
                let mut selected = if add || toggle {
                    self.selected_take_lanes.clone()
                } else {
                    Vec::new()
                };
                for lane in lanes {
                    match selected.iter().position(|held| *held == lane) {
                        Some(at) if toggle => {
                            selected.remove(at);
                        }
                        Some(_) => {}
                        None => selected.push(lane),
                    }
                }
                self.select_take_lane_headers(doc, selected);
                if let Some(last) = self.selected_take_lanes.last().copied()
                    && let Some(track) = doc.take_lane_track(last)
                {
                    self.show_track(doc, track);
                }
                Ok(())
            }
            ids::SHOW_TAKE_LANES => {
                let ShowTakeLanesArgs {
                    tracks,
                    on,
                    title_bars,
                } = parse(args)?;
                let tracks = self.targets(doc, tracks)?;
                let tracks: Vec<TrackId> = tracks
                    .into_iter()
                    .filter(|track| holds_takes(doc, *track))
                    .collect();
                let Some(first) = tracks.first().copied() else {
                    return Err(ViewError::Unavailable(
                        "no audio or MIDI track is selected".to_owned(),
                    ));
                };
                let show = on.unwrap_or_else(|| !self.shows_take_lanes(first));
                for track in &tracks {
                    if show {
                        self.show_take_lanes_of(*track);
                    } else {
                        self.shown_take_lanes.remove(track);
                    }
                }
                if title_bars {
                    self.selected = tracks;
                    self.title_bars = true;
                    self.selected_take_lanes.clear();
                }
                self.clamp_scroll(doc);
                Ok(())
            }
            other => Err(ViewError::InvalidArgs(format!(
                "`{other}` isn't a take lane view command"
            ))),
        }
    }
}
