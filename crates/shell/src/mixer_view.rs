//! What the mixers show (WP-1.9): whether the Session View's and the
//! Arrangement View's mixers are shown, how far each is scrolled, which
//! sections show, and which racks' chains are folded out as strips in
//! them, changed by the `mixer_view.*` commands. View state only:
//! nothing here is in the set, and no command here is an undo step (the
//! mixer view spec, R11 to R14, R16).

use std::collections::BTreeSet;

use bl_model::{Device, DeviceId, Document, RackKind, TrackId};
use serde_json::Value;

pub use bl_shell_spec::mixer_view::{
    ChainStrip, COLUMN_WIDTH, MixerOf, MixerViewState, RackFold, ScrollArgs, SectionArgs,
    ToggleArgs, ViewMixerState,
};
use bl_shell_spec::mixer_view::{
    CROSSFADER, FOCUS, FOLD_CHAINS, IN_OUT, RETURN_TRACKS, SCENE_TEMPO, SCROLL, SENDS, STATE,
    TOGGLE, TRACK_OPTIONS, VOLUME,
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
    in_out: bool,
    sends: bool,
    volume: bool,
    crossfader: bool,
    scene_tempo: bool,
    return_tracks: bool,
    track_options: bool,
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
            session: ViewMixerState {
                shown: true,
                scroll: 0.0,
            },
            arrangement: ViewMixerState {
                shown: false,
                scroll: 0.0,
            },
            in_out: true,
            sends: true,
            volume: true,
            crossfader: false,
            scene_tempo: false,
            return_tracks: true,
            track_options: false,
            folded: BTreeSet::new(),
            focus_requests: 0,
        }
    }

    /// Back to a new set's mixers, keeping the focus count.
    pub fn reset(&mut self) {
        *self = Self {
            focus_requests: self.focus_requests,
            ..Self::new()
        };
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

    /// Whether the In/Out section shows.
    #[must_use]
    pub fn in_out(&self) -> bool {
        self.in_out
    }

    /// Whether the Sends section shows.
    #[must_use]
    pub fn sends(&self) -> bool {
        self.sends
    }

    /// Whether the strips show.
    #[must_use]
    pub fn volume(&self) -> bool {
        self.volume
    }

    /// Whether the Crossfader section shows.
    #[must_use]
    pub fn crossfader(&self) -> bool {
        self.crossfader
    }

    /// Whether the Main column shows the scene tempo and time signature
    /// fields.
    #[must_use]
    pub fn scene_tempo(&self) -> bool {
        self.scene_tempo
    }

    /// Whether the return tracks' columns show.
    #[must_use]
    pub fn return_tracks(&self) -> bool {
        self.return_tracks
    }

    /// Whether the tracks' Track Options sections show.
    #[must_use]
    pub fn track_options(&self) -> bool {
        self.track_options
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
            in_out: self.in_out,
            sends: self.sends,
            volume: self.volume,
            crossfader: self.crossfader,
            scene_tempo: self.scene_tempo,
            return_tracks: self.return_tracks,
            track_options: self.track_options,
            racks,
            chains,
        }
    }

    /// The racks whose chains fold out in the mixers, and the chains folded
    /// out as strips, in the mixers' column order (racks spec, R27).
    ///
    /// A track's chains fold out when it holds an instrument or drum rack of
    /// more than one chain, and a chain's when it holds such a rack itself.
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
            let Some(rack) = t.devices().find(|d| foldable(d)) else {
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
            let nested = chain.devices.iter().find(|d| foldable(d));
            chains.push(ChainStrip {
                track,
                rack: rack.id,
                rack_name: rack.title().to_owned(),
                chain: chain.id,
                name: chain.name.clone(),
                volume: chain.mixer.volume,
                pan: chain.mixer.pan,
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

    /// Whether a switch-like command is on now, for a menu's check mark:
    /// the Mixer item for the view in `front`, and the sections.
    #[must_use]
    pub fn switch_state(&self, id: &str, front: MixerOf) -> Option<bool> {
        match id {
            TOGGLE => Some(self.of(front).shown),
            IN_OUT => Some(self.in_out),
            SENDS => Some(self.sends),
            VOLUME => Some(self.volume),
            CROSSFADER => Some(self.crossfader),
            SCENE_TEMPO => Some(self.scene_tempo),
            RETURN_TRACKS => Some(self.return_tracks),
            TRACK_OPTIONS => Some(self.track_options),
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
            IN_OUT | SENDS | VOLUME | CROSSFADER | SCENE_TEMPO | RETURN_TRACKS | TRACK_OPTIONS => {
                let SectionArgs { shown } = serde_json::from_value(args).map_err(parse_error)?;
                let section = match id {
                    IN_OUT => &mut self.in_out,
                    SENDS => &mut self.sends,
                    VOLUME => &mut self.volume,
                    CROSSFADER => &mut self.crossfader,
                    SCENE_TEMPO => &mut self.scene_tempo,
                    TRACK_OPTIONS => &mut self.track_options,
                    _ => &mut self.return_tracks,
                };
                *section = shown.unwrap_or(!*section);
            }
            SCROLL => {
                let ScrollArgs { view, x } = serde_json::from_value(args).map_err(parse_error)?;
                if !x.is_finite() {
                    return Err(MixerViewError::InvalidArgs(
                        "`x` must be a finite number".to_owned(),
                    ));
                }
                let columns = columns(doc, arrangement).len() + self.folds(doc, arrangement).1.len();
                let mixer = self.of_mut(view.unwrap_or(front));
                mixer.scroll = (mixer.scroll + x).clamp(0.0, max_scroll(columns));
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
                if !foldable(rack) {
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

    /// Keeps each scroll within the set's columns, after the set changed.
    /// The chain strips count as columns of their own (racks spec, R27).
    pub fn clamp_scroll(&mut self, doc: &Document, arrangement: &ArrangementView) {
        let (_, chains) = self.folds(doc, arrangement);
        let max = max_scroll(columns(doc, arrangement).len() + chains.len());
        for mixer in [&mut self.session, &mut self.arrangement] {
            mixer.scroll = mixer.scroll.min(max);
        }
    }
}

/// Whether a device's chains fold out in the mixers as strips (racks spec,
/// R27): an instrument or drum rack of more than one chain.
#[must_use]
pub fn foldable(rack: &Device) -> bool {
    matches!(rack.rack_kind(), Some(RackKind::Instrument | RackKind::Drum))
        && rack.chains.len() > 1
}

/// The furthest a mixer of `columns` track columns scrolls: until its last
/// track's column is the first in view.
#[must_use]
pub fn max_scroll(columns: usize) -> f64 {
    columns.saturating_sub(1) as f64 * f64::from(COLUMN_WIDTH)
}

/// The tracks with a column in the mixers, left to right: every track but
/// the returns and Main, in display order, without the tracks inside a
/// folded group (the mixer view spec, R1). Their activators are numbered
/// from 1 in this order, as the Arrangement's headers are.
#[must_use]
pub fn columns(doc: &Document, arrangement: &ArrangementView) -> Vec<TrackId> {
    shown_tracks(doc, |id| arrangement.is_folded(id))
        .map(|(id, _, _)| id)
        .collect()
}
