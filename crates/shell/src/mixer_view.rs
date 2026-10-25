//! What the mixers show (WP-1.9): whether the Session View's and the
//! Arrangement View's mixers are shown, how far each is scrolled, which
//! sections show, and which racks' chains are folded out as strips in
//! them, changed by the `mixer_view.*` commands. View state: no
//! command here is an undo step (the mixer view spec, R11 to R14, R16), and
//! only the columns' widths are saved with the set, beside the document
//! (R36).

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use bl_engine::TrackMeter;
use bl_model::{ColumnWidths, Device, DeviceId, Document, RackKind, TrackId, clamp_column_width};
use serde_json::Value;

pub use bl_shell_spec::mixer_view::{
    COLUMN_WIDTH, ChainStrip, ColumnWidth, EXTENDED_METERS_HEIGHT, MAIN_COLUMN_WIDTH,
    MIN_STRIP_HEIGHT, MeterValue, MixerOf, MixerViewState, PAN_HIDES_BELOW, PEAK_HOLD_SECONDS,
    RackFold, ResetPeakArgs, STRIP_HEIGHT, ScrollArgs, ScrollReturnsArgs, SectionArgs,
    SetColumnWidthArgs, SetHeightArgs, TICK_LABELS_WIDTH, ToggleArgs, TrackGainReduction,
    TrackImpact, TrackMeterValue, TrackPeak, ViewMixerState, ViewSectionArgs, impact_segments,
};
use bl_shell_spec::mixer_view::{
    CROSSFADER, DEVICES, FOCUS, FOLD_CHAINS, IN_OUT, PERFORMANCE_IMPACT, RESET_PEAK, RETURN_TRACKS,
    RETURN_TRACKS_SCROLL, SCENE_TEMPO, SCROLL, SENDS, SET_COLUMN_WIDTH, SET_HEIGHT, STATE, TOGGLE,
    TRACK_OPTIONS, VOLUME,
};

use crate::arrangement::ArrangementView;
use crate::arrangement::rows::shown_tracks;

/// Why a Mixer view command didn't run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MixerViewError {
    /// The arguments don't fit.
    InvalidArgs(String),
}

/// The mixers' view state. See the [module docs](self).
#[derive(Clone, Debug, PartialEq)]
pub struct MixerView {
    session: ViewMixerState,
    arrangement: ViewMixerState,
    scene_tempo: bool,
    /// Whether the return tracks scroll with the tracks (IMP-01, the mixer
    /// view spec R21). Off in a new install, so they stay pinned, as in
    /// Live.
    return_tracks_scroll: bool,
    /// The columns' widths where they aren't the default (R36), saved with
    /// the set.
    widths: ColumnWidths,
    /// Each track's held peak and peak lines (R33, R34).
    peaks: BTreeMap<TrackId, PeakHold>,
    /// The racks whose chains are folded out as strips in the mixers (racks
    /// spec, R27).
    folded: BTreeSet<DeviceId>,
    /// Counts `mixer_view.focus` runs, so the window moves the focus once
    /// for each.
    focus_requests: u64,
}

impl Default for MixerView {
    fn default() -> Self {
        Self::new()
    }
}

impl MixerView {
    /// A new set's mixers: the Session View's shown and the Arrangement's
    /// hidden, with In/Out, Sends, the strips, and the returns showing and
    /// the tracks' Track Options and the Crossfader sections hidden (the
    /// mixer view spec, R11, R12).
    #[must_use]
    pub fn new() -> Self {
        Self {
            session: ViewMixerState::new(true),
            arrangement: ViewMixerState::new(false),
            scene_tempo: false,
            return_tracks_scroll: false,
            widths: ColumnWidths::new(),
            peaks: BTreeMap::new(),
            folded: BTreeSet::new(),
            focus_requests: 0,
        }
    }

    /// Back to a new set's mixers, keeping the focus count and the
    /// application setting the return tracks scroll with the tracks.
    pub fn reset(&mut self) {
        *self = Self {
            focus_requests: self.focus_requests,
            return_tracks_scroll: self.return_tracks_scroll,
            ..Self::new()
        };
    }

    /// The columns' widths, as a save writes them (R36).
    #[must_use]
    pub fn widths(&self) -> &ColumnWidths {
        &self.widths
    }

    /// Gives the mixers the widths a set was saved with.
    pub fn restore_widths(&mut self, widths: ColumnWidths) {
        self.widths = widths;
    }

    /// The default width of `track`'s column: 94, and Main's 104.
    fn column_width_default(&self, doc: &Document, track: TrackId) -> f32 {
        if track == doc.main_id() {
            MAIN_COLUMN_WIDTH
        } else {
            COLUMN_WIDTH
        }
    }

    /// The width of `track`'s column in points at 100% (R36): the one it was
    /// resized to, or 94, and Main's 104.
    #[must_use]
    pub fn column_width(&self, doc: &Document, track: TrackId) -> f32 {
        self.widths
            .get(&track)
            .copied()
            .unwrap_or_else(|| self.column_width_default(doc, track))
    }

    /// Takes the peaks the engine published, at `now`: each track's held
    /// peak keeps the highest one until it is reset, and each channel's
    /// peak line holds for two seconds before it falls to the current peak
    /// (R33, R34).
    pub fn observe_peaks(&mut self, meters: &[TrackMeter], now: Instant) {
        let mut before = std::mem::take(&mut self.peaks);
        for meter in meters {
            let held = before.remove(&meter.track).unwrap_or(PeakHold {
                peak: 0.0,
                current: [0.0; 2],
                line: [0.0; 2],
                since: [now; 2],
            });
            self.peaks
                .insert(meter.track, held.observe(meter.peak, now));
        }
    }

    /// Each track's held peak and peak lines, for `mixer_view.state`.
    #[must_use]
    pub fn peaks(&self) -> Vec<TrackPeak> {
        self.peaks
            .iter()
            .map(|(track, held)| TrackPeak {
                track: *track,
                peak: held.peak,
                hold: held.line,
            })
            .collect()
    }

    /// The highest peak `track` has had since its readout was last reset
    /// (R33), as a linear amplitude.
    #[must_use]
    pub fn held_peak(&self, track: TrackId) -> f32 {
        self.peaks.get(&track).map_or(0.0, |held| held.peak)
    }

    /// The line `track`'s meter marks each channel's highest recent peak
    /// with (R34), as a linear amplitude.
    #[must_use]
    pub fn peak_line(&self, track: TrackId) -> [f32; 2] {
        self.peaks.get(&track).map_or([0.0; 2], |held| held.line)
    }

    /// One view's mixer.
    #[must_use]
    pub fn of(&self, view: MixerOf) -> ViewMixerState {
        match view {
            MixerOf::Session => self.session,
            MixerOf::Arrangement => self.arrangement,
        }
    }

    fn of_mut(&mut self, view: MixerOf) -> &mut ViewMixerState {
        match view {
            MixerOf::Session => &mut self.session,
            MixerOf::Arrangement => &mut self.arrangement,
        }
    }

    /// Whether the Main column shows the scene tempo and time signature
    /// fields.
    #[must_use]
    pub fn scene_tempo(&self) -> bool {
        self.scene_tempo
    }

    /// Whether the return tracks scroll with the tracks (IMP-01, the mixer
    /// view spec R21).
    #[must_use]
    pub fn return_tracks_scroll(&self) -> bool {
        self.return_tracks_scroll
    }

    /// Sets whether the return tracks scroll with the tracks, as the
    /// application setting the preferences keep does.
    pub fn set_return_tracks_scroll(&mut self, on: bool) {
        self.return_tracks_scroll = on;
    }

    /// How many times `mixer_view.focus` has run.
    #[must_use]
    pub fn focus_requests(&self) -> u64 {
        self.focus_requests
    }

    /// The state `mixer_view.state` reports: what the mixers show, and the
    /// rack chains folded out in them (racks spec, R27).
    #[must_use]
    pub fn state(&self, doc: &Document, arrangement: &ArrangementView) -> MixerViewState {
        let (racks, chains) = self.folds(doc, arrangement);
        MixerViewState {
            session: self.session,
            arrangement: self.arrangement,
            scene_tempo: self.scene_tempo,
            return_tracks_scroll: self.return_tracks_scroll,
            performance_impact_values: Vec::new(),
            gain_reduction: Vec::new(),
            meters: Vec::new(),
            peaks: self.peaks(),
            column_widths: self.column_widths(doc, arrangement),
            racks,
            chains,
        }
    }

    /// The racks whose chains fold out in the mixers, and the chains folded
    /// out as strips, in the mixers' column order (racks spec, R27).
    ///
    /// A track's chains fold out when it holds an instrument or drum rack of
    /// more than one chain, and a chain's when it holds a rack of more than
    /// one chain (racks spec, R27).
    #[must_use]
    pub fn folds(
        &self,
        doc: &Document,
        arrangement: &ArrangementView,
    ) -> (Vec<RackFold>, Vec<ChainStrip>) {
        let mut racks = Vec::new();
        let mut chains = Vec::new();
        for track in columns(doc, arrangement) {
            let Some(t) = doc.track(track) else {
                continue;
            };
            let Some(rack) = t.devices().find(|d| shows_chains(d)) else {
                continue;
            };
            racks.push(self.fold_of(track, rack));
            if self.folded.contains(&rack.id) {
                self.strip_of(track, rack, &mut chains);
            }
        }
        (racks, chains)
    }

    /// One rack's fold, as the state reports it.
    fn fold_of(&self, track: TrackId, rack: &Device) -> RackFold {
        RackFold {
            track,
            rack: rack.id,
            name: rack.title().to_owned(),
            folded: self.folded.contains(&rack.id),
        }
    }

    /// A rack's chains as strips, and the chains of the racks inside them.
    fn strip_of(&self, track: TrackId, rack: &Device, chains: &mut Vec<ChainStrip>) {
        for chain in &rack.chains {
            let nested = chain.devices.iter().find(|d| chains_fold(d));
            chains.push(ChainStrip {
                track,
                rack: rack.id,
                rack_name: rack.title().to_owned(),
                chain: chain.id,
                name: chain.name.clone(),
                volume: chain.mixer.volume,
                pan: chain.mixer.pan,
                has_mixer: rack
                    .rack_kind()
                    .is_some_and(|kind| kind.chains_have_volume_and_pan()),
                enabled: chain.mixer.enabled,
                solo: chain.mixer.solo,
                colour: chain.colour,
                nested: nested.map(|d| self.fold_of(track, d)),
            });
            if let Some(nested) = nested
                && self.folded.contains(&nested.id)
            {
                self.strip_of(track, nested, chains);
            }
        }
    }

    /// Every column's width, in the order the columns run: the tracks, the
    /// return tracks, Main (R36, R38).
    fn column_widths(&self, doc: &Document, arrangement: &ArrangementView) -> Vec<ColumnWidth> {
        columns(doc, arrangement)
            .into_iter()
            .chain(doc.returns())
            .chain(std::iter::once(doc.main_id()))
            .map(|track| ColumnWidth {
                track,
                width: self.column_width(doc, track),
            })
            .collect()
    }

    /// Whether a switch-like command is on now, for a menu's check mark:
    /// the Mixer item for the view in `front`, and the sections, as that
    /// view's mixer shows them (R37).
    #[must_use]
    pub fn switch_state(&self, id: &str, front: MixerOf) -> Option<bool> {
        let mixer = self.of(front);
        match id {
            TOGGLE => Some(mixer.shown),
            IN_OUT => Some(mixer.in_out),
            SENDS => Some(mixer.sends),
            VOLUME => Some(mixer.volume),
            CROSSFADER => Some(mixer.crossfader),
            SCENE_TEMPO => Some(self.scene_tempo),
            RETURN_TRACKS => Some(mixer.return_tracks),
            RETURN_TRACKS_SCROLL => Some(self.return_tracks_scroll),
            TRACK_OPTIONS => Some(mixer.track_options),
            PERFORMANCE_IMPACT => Some(mixer.performance_impact),
            DEVICES => Some(mixer.devices),
            _ => None,
        }
    }

    /// Runs a Mixer view command. `front` is the view in front, which the
    /// commands act on unless they name one; `doc` and `arrangement` give
    /// the columns a scroll stays within.
    ///
    /// # Errors
    ///
    /// [`MixerViewError`] if the arguments don't fit.
    pub fn run(
        &mut self,
        id: &str,
        args: Value,
        front: MixerOf,
        doc: &Document,
        arrangement: &ArrangementView,
    ) -> Result<MixerViewState, MixerViewError> {
        let args = if args.is_null() {
            Value::Object(serde_json::Map::new())
        } else {
            args
        };
        let parse_error = |e: serde_json::Error| MixerViewError::InvalidArgs(e.to_string());
        match id {
            STATE => {
                let _: bl_commands::NoArgs = serde_json::from_value(args).map_err(parse_error)?;
            }
            TOGGLE => {
                let ToggleArgs { view, shown } =
                    serde_json::from_value(args).map_err(parse_error)?;
                let mixer = self.of_mut(view.unwrap_or(front));
                mixer.shown = shown.unwrap_or(!mixer.shown);
            }
            SCENE_TEMPO => {
                let SectionArgs { shown } = serde_json::from_value(args).map_err(parse_error)?;
                self.scene_tempo = shown.unwrap_or(!self.scene_tempo);
            }
            IN_OUT | SENDS | VOLUME | CROSSFADER | RETURN_TRACKS | TRACK_OPTIONS
            | PERFORMANCE_IMPACT | DEVICES => {
                let ViewSectionArgs { view, shown } =
                    serde_json::from_value(args).map_err(parse_error)?;
                let mixer = self.of_mut(view.unwrap_or(front));
                let section = match id {
                    IN_OUT => &mut mixer.in_out,
                    SENDS => &mut mixer.sends,
                    VOLUME => &mut mixer.volume,
                    CROSSFADER => &mut mixer.crossfader,
                    TRACK_OPTIONS => &mut mixer.track_options,
                    PERFORMANCE_IMPACT => &mut mixer.performance_impact,
                    DEVICES => &mut mixer.devices,
                    _ => &mut mixer.return_tracks,
                };
                *section = shown.unwrap_or(!*section);
            }
            SET_HEIGHT => {
                let SetHeightArgs { view, height } =
                    serde_json::from_value(args).map_err(parse_error)?;
                if !height.is_finite() {
                    return Err(MixerViewError::InvalidArgs(
                        "`height` must be a finite number".to_owned(),
                    ));
                }
                self.of_mut(view.unwrap_or(front)).height = height.max(MIN_STRIP_HEIGHT);
            }
            SET_COLUMN_WIDTH => {
                let SetColumnWidthArgs { track, width } =
                    serde_json::from_value(args).map_err(parse_error)?;
                if !width.is_finite() {
                    return Err(MixerViewError::InvalidArgs(
                        "`width` must be a finite number".to_owned(),
                    ));
                }
                let targets = match track {
                    Some(track) if doc.track(track).is_some() => vec![track],
                    Some(track) => {
                        return Err(MixerViewError::InvalidArgs(format!("no track {track}")));
                    }
                    None => columns(doc, arrangement),
                };
                let width = clamp_column_width(width);
                for track in targets {
                    // A column at its default width has no entry, as a set
                    // saved before columns could be resized has none.
                    if (width - self.column_width_default(doc, track)).abs() < f32::EPSILON {
                        self.widths.remove(&track);
                    } else {
                        self.widths.insert(track, width);
                    }
                }
            }
            RESET_PEAK => {
                let ResetPeakArgs { track } = serde_json::from_value(args).map_err(parse_error)?;
                if let Some(track) = track
                    && doc.track(track).is_none()
                {
                    return Err(MixerViewError::InvalidArgs(format!("no track {track}")));
                }
                for (held_track, held) in &mut self.peaks {
                    if track.is_none_or(|t| t == *held_track) {
                        held.reset();
                    }
                }
            }
            RETURN_TRACKS_SCROLL => {
                let ScrollReturnsArgs { on } = serde_json::from_value(args).map_err(parse_error)?;
                self.return_tracks_scroll = on.unwrap_or(!self.return_tracks_scroll);
            }
            SCROLL => {
                let ScrollArgs { view, x } = serde_json::from_value(args).map_err(parse_error)?;
                if !x.is_finite() {
                    return Err(MixerViewError::InvalidArgs(
                        "`x` must be a finite number".to_owned(),
                    ));
                }
                let max = self.max_scroll(doc, arrangement);
                let mixer = self.of_mut(view.unwrap_or(front));
                mixer.scroll = (mixer.scroll + x).clamp(0.0, max);
            }
            FOCUS => {
                let _: bl_commands::NoArgs = serde_json::from_value(args).map_err(parse_error)?;
                self.arrangement.shown = true;
                self.focus_requests = self.focus_requests.wrapping_add(1);
            }
            FOLD_CHAINS => {
                let args: bl_shell_spec::mixer_view::FoldChainsArgs =
                    serde_json::from_value(args).map_err(parse_error)?;
                let Some(rack) = doc.device(args.rack) else {
                    return Err(MixerViewError::InvalidArgs(format!(
                        "no device {}",
                        args.rack
                    )));
                };
                if !chains_fold(rack) {
                    return Err(MixerViewError::InvalidArgs(format!(
                        "{} has no chains to fold out",
                        args.rack
                    )));
                }
                let shown = args.shown.unwrap_or(!self.folded.contains(&args.rack));
                if shown {
                    self.folded.insert(args.rack);
                } else {
                    self.folded.remove(&args.rack);
                }
            }
            _ => {
                return Err(MixerViewError::InvalidArgs(format!(
                    "`{id}` isn't a Mixer view command"
                )));
            }
        }
        Ok(self.state(doc, arrangement))
    }

    /// The furthest the mixers scroll (R14): see [`max_scroll`].
    fn max_scroll(&self, doc: &Document, arrangement: &ArrangementView) -> f64 {
        let (_, chains) = self.folds(doc, arrangement);
        let mut tracks = Vec::new();
        for track in columns(doc, arrangement) {
            tracks.push(self.column_width(doc, track));
            let strips = chains.iter().filter(|c| c.track == track).count();
            tracks.extend(std::iter::repeat_n(COLUMN_WIDTH, strips));
        }
        let returns: Vec<f32> = doc
            .returns()
            .map(|track| self.column_width(doc, track))
            .collect();
        max_scroll(&tracks, &returns, self.return_tracks_scroll)
    }

    /// Keeps each scroll within the set's columns, after the set changed.
    /// The chain strips count as columns of their own (racks spec, R27).
    pub fn clamp_scroll(&mut self, doc: &Document, arrangement: &ArrangementView) {
        let max = self.max_scroll(doc, arrangement);
        for mixer in [&mut self.session, &mut self.arrangement] {
            mixer.scroll = mixer.scroll.min(max);
        }
    }
}

/// Whether a device's chains fold out in the mixers as strips (racks spec,
/// R27): an instrument or drum rack of more than one chain.
#[must_use]
pub fn shows_chains(rack: &Device) -> bool {
    matches!(
        rack.rack_kind(),
        Some(RackKind::Instrument | RackKind::Drum)
    ) && rack.chains.len() > 1
}

/// Whether a rack's chains fold out in the mixers as strips (racks spec,
/// R27): a rack of more than one chain.
#[must_use]
pub fn chains_fold(rack: &Device) -> bool {
    rack.chains.len() > 1
}

/// The furthest a mixer whose track columns are `tracks` points wide scrolls:
/// until its last track's column is the first in view. With the return
/// tracks scrolling with the tracks (IMP-01), the returns, `returns` points
/// wide, are columns of their own in the scroll, so the scroll reaches the
/// last return.
#[must_use]
pub fn max_scroll(tracks: &[f32], returns: &[f32], returns_scroll: bool) -> f64 {
    let sum = |widths: &[f32]| widths.iter().map(|w| f64::from(*w)).sum::<f64>();
    if returns_scroll {
        sum(tracks) + sum(returns)
    } else {
        sum(tracks) - tracks.last().map_or(0.0, |w| f64::from(*w))
    }
}

/// A track's held peak and the lines its meter's channels mark their highest
/// recent peaks with (R33, R34).
#[derive(Clone, Copy, Debug, PartialEq)]
struct PeakHold {
    /// The highest peak since the readout was last reset.
    peak: f32,
    /// The channels' current peaks, as the engine last published them.
    current: [f32; 2],
    /// Each channel's peak line.
    line: [f32; 2],
    /// When each line last rose to a peak.
    since: [Instant; 2],
}

impl PeakHold {
    /// Takes the channels' peaks at `now`.
    fn observe(mut self, peak: [f32; 2], now: Instant) -> Self {
        self.current = peak;
        self.peak = self.peak.max(peak[0]).max(peak[1]);
        for (channel, level) in peak.into_iter().enumerate() {
            let held = now.duration_since(self.since[channel]).as_secs_f64();
            if level >= self.line[channel] || held >= PEAK_HOLD_SECONDS {
                self.line[channel] = level;
                self.since[channel] = now;
            }
        }
        self
    }

    /// Sets the held peak back to the current one.
    fn reset(&mut self) {
        self.peak = self.current[0].max(self.current[1]);
    }
}

/// The tracks with a column in the mixers, left to right: every track but
/// the returns and Main, in display order, without the tracks inside a
/// folded group (the mixer view spec, R1). Their activators are numbered
/// from 1 in this order, as the Arrangement's headers are.
#[must_use]
pub fn columns(doc: &Document, arrangement: &ArrangementView) -> Vec<TrackId> {
    shown_tracks(doc, arrangement)
        .map(|(id, _, _)| id)
        .collect()
}
