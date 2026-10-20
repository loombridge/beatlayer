//! What the mixers show (WP-1.9): whether the Session View's and the
//! Arrangement View's mixers are shown, how far each is scrolled, and which
//! sections show, changed by the `mixer_view.*` commands. View state only:
//! nothing here is in the set, and no command here is an undo step (the
//! mixer view spec, R11 to R14, R16).

use bl_model::{Document, TrackId};
use serde_json::Value;

pub use bl_shell_spec::mixer_view::{
    COLUMN_WIDTH, MixerOf, MixerViewState, ScrollArgs, SectionArgs, ToggleArgs, ViewMixerState,
};
use bl_shell_spec::mixer_view::{
    CROSSFADER, FOCUS, IN_OUT, RETURN_TRACKS, SCENE_TEMPO, SCROLL, SENDS, STATE, TOGGLE,
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
    in_out: bool,
    sends: bool,
    volume: bool,
    crossfader: bool,
    scene_tempo: bool,
    return_tracks: bool,
    track_options: bool,
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

    /// The state `mixer_view.state` reports.
    #[must_use]
    pub fn state(&self) -> MixerViewState {
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
                let columns = columns(doc, arrangement).len();
                let mixer = self.of_mut(view.unwrap_or(front));
                mixer.scroll = (mixer.scroll + x).clamp(0.0, max_scroll(columns));
            }
            FOCUS => {
                let _: bl_commands::NoArgs = serde_json::from_value(args).map_err(parse_error)?;
                self.arrangement.shown = true;
                self.focus_requests = self.focus_requests.wrapping_add(1);
            }
            _ => {
                return Err(MixerViewError::InvalidArgs(format!(
                    "`{id}` isn't a Mixer view command"
                )));
            }
        }
        Ok(self.state())
    }

    /// Keeps each scroll within the set's columns, after the set changed.
    pub fn clamp_scroll(&mut self, doc: &Document, arrangement: &ArrangementView) {
        let max = max_scroll(columns(doc, arrangement).len());
        for mixer in [&mut self.session, &mut self.arrangement] {
            mixer.scroll = mixer.scroll.min(max);
        }
    }
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
