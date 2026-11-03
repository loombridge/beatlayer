//! The Mixer's columns and meters.

use super::{MainView, Shell};
use crate::mixer_view::{MixerOf, TrackMeterValue};
use bl_engine::{MeterOwner, MeterSite, PointReading};
use bl_model::TrackId;
use bl_shell_spec::mixer_view::MeterValue;

use super::{Area, ShellError, WindowKind, to_value};
use crate::mixer_view::MixerViewError;
use bl_shell_spec::mixer_view as mixer_ids;
use serde_json::Value;

impl Shell {
    /// The mixer of the view in the window in front.
    #[must_use]
    pub fn front_mixer(&self) -> MixerOf {
        self.mixer_of(self.acting_window())
    }

    /// The mixer of the view `window` shows.
    #[must_use]
    pub fn mixer_of(&self, window: WindowKind) -> MixerOf {
        match self.main_view_of(window) {
            MainView::Arrangement => MixerOf::Arrangement,
            MainView::Session => MixerOf::Session,
        }
    }

    /// The tracks with a column in the mixers, left to right (the mixer
    /// view spec, R1).
    #[must_use]
    pub fn mixer_columns(&self) -> Vec<TrackId> {
        crate::mixer_view::columns(self.editor.document(), &self.arrangement)
    }

    /// The meters the engine last published, as [`Shell::poll_engine`] took
    /// them: one per track, in the order the tracks run.
    #[must_use]
    pub fn meters(&self) -> &[bl_engine::TrackMeter] {
        &self.meters
    }

    /// One track's meter, if the engine published one.
    #[must_use]
    pub fn meter(&self, track: TrackId) -> Option<&bl_engine::TrackMeter> {
        self.meters.iter().find(|meter| meter.track == track)
    }

    /// Each track's Performance Impact, as the engine last published it:
    /// the track's share of the set's processing time and how many of the
    /// six segments it lights (the mixer spec, R31).
    #[must_use]
    pub fn impact_values(&self) -> Vec<bl_shell_spec::mixer_view::TrackImpact> {
        self.meters
            .iter()
            .map(|meter| bl_shell_spec::mixer_view::TrackImpact {
                track: meter.track,
                segments: bl_shell_spec::mixer_view::impact_segments(meter.impact),
                share: meter.impact,
            })
            .collect()
    }

    /// One track's gain reduction in dB, as its compressors and limiters
    /// last reported it: zero when the engine published no meter for the
    /// track, or none of them reduced (the mixer view spec, R40).
    #[must_use]
    pub fn gain_reduction(&self, track: TrackId) -> f32 {
        self.meter(track).map_or(0.0, |meter| meter.gain_reduction)
    }

    /// Every track's gain reduction, as `mixer_view.state` reports it: one
    /// entry per track the engine published a meter for, in the order the
    /// tracks run (IMP-25, the mixer view spec R40).
    #[must_use]
    pub fn gain_reductions(&self) -> Vec<crate::mixer_view::TrackGainReduction> {
        self.meters
            .iter()
            .map(|meter| crate::mixer_view::TrackGainReduction {
                track: meter.track,
                reduction: meter.gain_reduction,
            })
            .collect()
    }

    /// What the engine last published at the meter `at` of the chain
    /// `owner` (the MIDI meters spec, R2).
    #[must_use]
    pub fn chain_meter(
        &self,
        owner: bl_engine::MeterOwner,
        at: bl_engine::MeterSite,
    ) -> Option<MeterValue> {
        self.chain_meters
            .iter()
            .find(|meter| meter.owner == owner && meter.at == at)
            .map(|meter| match meter.reading {
                PointReading::Midi {
                    velocity,
                    channel,
                    expression,
                } => MeterValue::Midi {
                    velocity,
                    channel,
                    expression,
                },
                PointReading::Audio { peak } => MeterValue::Level { peak },
            })
    }

    /// Whether any meter between devices shows something, so the window
    /// keeps redrawing (the MIDI meters spec, R7).
    #[must_use]
    pub fn chain_meters_lit(&self) -> bool {
        self.chain_meters.iter().any(|meter| match meter.reading {
            PointReading::Midi {
                velocity,
                channel,
                expression,
            } => velocity > 0.0 || channel > 0.0 || expression > 0.0,
            PointReading::Audio { peak } => peak.iter().any(|level| *level > 0.0),
        })
    }

    /// Every track's meter as `mixer_view.state` reports it (the MIDI meters
    /// spec, R1 and R6): a MIDI meter for a MIDI track that holds no
    /// instrument and no audio effect, a level meter for the rest.
    #[must_use]
    pub fn track_meter_values(&self) -> Vec<TrackMeterValue> {
        let doc = self.editor.document();
        self.meters
            .iter()
            .filter_map(|meter| {
                let midi = doc.track(meter.track)?.shows_midi_meter();
                let value = if midi {
                    self.chain_meter(MeterOwner::Track(meter.track), MeterSite::End)?
                } else {
                    MeterValue::Level { peak: meter.peak }
                };
                Some(TrackMeterValue {
                    track: meter.track,
                    value,
                })
            })
            .collect()
    }

    /// Sets the meters between devices, as [`Shell::poll_engine`] does with
    /// the engine's: for tests with no engine running.
    pub fn set_chain_meters(&mut self, meters: &[bl_engine::ChainMeter]) {
        self.chain_meters.clear();
        self.chain_meters.extend_from_slice(meters);
    }

    /// Sets the meters the windows draw, as [`Shell::poll_engine`] does with
    /// the engine's: for benchmarks and tests with no engine running (while
    /// one runs, the next poll takes its meters instead).
    pub fn set_meters(&mut self, meters: &[bl_engine::TrackMeter]) {
        self.meters.clear();
        self.meters.extend_from_slice(meters);
        self.observe_peaks();
    }

    /// Hands the mixers the meters as they stand, with the time the shell's
    /// clock reads (the test clock's, when there is one), for the peaks they
    /// hold (the mixer view spec, R33, R34).
    pub(super) fn observe_peaks(&mut self) {
        let now = self.midi.now();
        self.mixer.observe_peaks(&self.meters, now);
    }

    /// Sets the chains processing, for a host with no engine and for tests;
    /// a running engine's replace them.
    pub fn set_processing(&mut self, processing: Vec<bl_model::ChainId>) {
        self.processing = processing;
    }

    /// Sets the parameters under modulation, for a host with no engine and
    /// for tests; a running engine's replace them.
    pub fn set_modulated(&mut self, modulated: Vec<bl_engine::PlayedParam>) {
        self.modulated = modulated;
    }

    /// Sets the parameters an instrument's matrix moved, for a host with no
    /// engine and for tests; a running engine's replace them.
    pub fn set_matrix_played(&mut self, played: Vec<bl_engine::PlayedParam>) {
        self.matrix_played = played;
    }

    /// Runs the Mixer View's commands.
    pub(super) fn run_mixer_view(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let front = self.front_mixer();
        let result = self
            .mixer
            .run(id, args, front, self.editor.document(), &self.arrangement)
            .map_err(
                |MixerViewError::InvalidArgs(message)| ShellError::InvalidArgs {
                    command: id.to_owned(),
                    message,
                },
            )
            .map(|mut state| {
                // Spec: mixer-view R40. The strips' gain-reduction bars
                // read with the meters.
                state.gain_reduction = self.gain_reductions();
                // Spec: mixer R31. The Performance Impact values the
                // engine last published, one per track.
                state.performance_impact_values = self.impact_values();
                // Spec: midi-meters R6. Each track's meter, MIDI or level.
                state.meters = self.track_meter_values();
                state
            });
        if result.is_ok() && id == mixer_ids::FOCUS {
            // Spec: mixer-view R13.
            self.focus_area(Area::MainView);
        }
        if result.is_ok() && id == mixer_ids::RETURN_TRACKS_SCROLL {
            // Spec: mixer-view R21. The switch is an application
            // setting, kept between launches, and the Arrangement's
            // lanes follow it.
            let on = self.mixer.return_tracks_scroll();
            self.prefs.return_tracks_scroll = on;
            self.arrangement.set_returns_scroll(on);
            self.save_preferences();
        }
        if id != mixer_ids::STATE {
            self.touch();
        }
        result.map(|state| to_value(&state))
    }
}
