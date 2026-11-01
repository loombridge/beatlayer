//! Tap Tempo: `transport.tap_tempo` and the start a run of taps can make
//! (the control bar transport spec, R18 to R22). The law is in
//! [`crate::tap_tempo`].

use super::{Shell, ShellError, to_value};
use crate::tap_tempo::{Tapped, tempo_of};
use crate::transport::{self, Group};
use bl_core::time::{Bpm, TempoMap};
use bl_engine::TransportCommand;
use bl_model::{Command, Maker};
use bl_shell_spec::transport::{self as ids, TapTempo, TapTempoArgs};
use serde_json::{Value, json};

impl Shell {
    /// The command's refusal: `reason`.
    fn tap_refused(reason: impl Into<String>) -> ShellError {
        ShellError::Unavailable {
            command: ids::TAP_TEMPO.to_owned(),
            reason: reason.into(),
        }
    }

    /// Runs `transport.tap_tempo`: counts the tap in the run, sets the
    /// tempo from it, and starts the transport a beat later if the run is
    /// the one that starts it.
    pub(super) fn run_tap_tempo(&mut self, args: Value) -> Result<Value, ShellError> {
        let TapTempoArgs { at } = Self::args(ids::TAP_TEMPO, args)?;
        // Spec: tempo-follower R21. Tempo Follower sets the tempo.
        if self.tempo_follow_on {
            return Err(Self::tap_refused(
                "Tempo Follower is on and sets the tempo; switch it off to tap one",
            ));
        }
        if !transport::editable(self, Group::Tempo) {
            return Err(Self::tap_refused(
                "the song has tempo changes, or a clip leads its tempo; the taps could not set it",
            ));
        }
        // The click's moment, taken onto the engine clock as MIDI input is.
        let (clock, rate) = self
            .audio
            .clock_frame_at(self.midi.now())
            .map_err(Self::tap_refused)?;
        let frame = at
            .or(clock)
            .ok_or_else(|| Self::tap_refused("the audio engine has not played a block yet"))?;
        let Tapped::Counted { mean_interval } = self.tap.tap(frame, rate) else {
            return Ok(to_value(&self.tap.report(frame, rate)));
        };
        if let Some(mean) = mean_interval {
            let signature = transport::signature(self.editor.document(), self);
            let bpm = tempo_of(mean, rate, signature.denominator());
            self.set_tapped_tempo(bpm)?;
            self.tap.tempo = Some(bpm);
            if self.start_transport_with_tap_tempo()
                && self.tap.taps() == u32::from(signature.numerator())
                && !self.audio.transport_state().playing
            {
                // One beat after the tap, where the next tap would fall.
                let start = frame + mean.round() as u64;
                self.audio
                    .send_transport_at(TransportCommand::Play, start)
                    .map_err(Self::tap_refused)?;
                self.tap.start_at = Some(start);
            }
        }
        Ok(to_value(&self.tap.report(frame, rate)))
    }

    /// Sets the song's tempo to `bpm` as the run's one undo step, named Tap
    /// Tempo: the run's first edit records it, and each tap after amends it
    /// while it's still the newest step (R21).
    fn set_tapped_tempo(&mut self, bpm: f64) -> Result<(), ShellError> {
        let tempo = TempoMap::constant(Bpm::new(bpm).map_err(|error| ShellError::Failed {
            command: ids::TAP_TEMPO.to_owned(),
            message: error.to_string(),
        })?);
        let index = self.editor.current_index();
        let amend = self.tap.step == Some((self.document_revision, index));
        if amend {
            self.editor
                .apply_into_newest(Command::SetTempo { tempo })
                .map_err(|error| ShellError::Failed {
                    command: ids::TAP_TEMPO.to_owned(),
                    message: error.to_string(),
                })?;
            self.dirty = true;
            self.document_changed();
            // Spec: crash-recovery R1. The amended step is journalled too.
            self.journal_edit(Some("Tap Tempo"));
            self.touch();
        } else {
            self.run_labelled(
                Maker::Person,
                "Tap Tempo",
                ids::SET_TEMPO,
                json!({ "tempo": tempo }),
            )?;
        }
        // The run's step is the newest one only if this tap left it there: an
        // amend keeps the step, a first edit records one.
        let now = self.editor.current_index();
        let kept = if amend { now == index } else { now > index };
        self.tap.step = kept.then_some((self.document_revision, now));
        Ok(())
    }

    /// The run of taps as `transport.state` reports it.
    pub(crate) fn tap_tempo_report(&self, clock: u64) -> TapTempo {
        let rate = self.audio.sample_rate().unwrap_or(0);
        self.tap.report(clock, rate)
    }

    /// The tempo the run of taps set, once it has one: the Tap Tempo
    /// button describes it.
    #[must_use]
    pub fn tap_tempo_tempo(&self) -> Option<f64> {
        self.tap.tempo
    }

    /// A Stop abandons the start a run of taps was waiting for.
    pub(super) fn cancel_tap_start(&mut self) {
        self.tap.start_at = None;
    }
}
