//! Tempo Follower: switching Follow, its two settings, its state, and the
//! followed tempo in the set (the Tempo Follower spec, R1 to R6, R19, R21).

use std::time::Instant;

use super::{Shell, ShellError, to_value};
use bl_commands::NoArgs;
use bl_model::Channel;
use bl_shell_spec::tempo_follower as ids;
use serde_json::Value;

impl Shell {
    /// Whether Follow is on: the tracker listens and the song's tempo follows
    /// it (R3).
    #[must_use]
    pub fn tempo_follower_on(&self) -> bool {
        self.tempo_follow_on
    }

    /// Whether the Control Bar shows the Follow toggle (R1, R2).
    #[must_use]
    pub fn tempo_follower_toggle_shown(&self) -> bool {
        self.prefs.tempo_follower.show_toggle
    }

    /// The audio input Tempo Follower listens to (R1).
    #[must_use]
    pub fn tempo_follower_input(&self) -> Channel {
        self.prefs.tempo_follower.input
    }

    /// Why Follow can't run, while it can't: the audio device's input can't
    /// be reached (R4).
    #[must_use]
    pub fn tempo_follower_unavailable(&self) -> Option<String> {
        if let Some(reason) = self.audio.engine_refusal() {
            return Some(reason);
        }
        let (inputs, _) = self.audio.channels();
        if inputs == 0 {
            return Some("the audio device's inputs are off".to_owned());
        }
        let channel = self.tempo_follower_input();
        (!crate::tempo_follower::reachable(channel, inputs))
            .then(|| format!("the audio device has no input channel {}", channel.label()))
    }

    /// The tempo the song follows, while Follow is on and the tracker has
    /// been confident: what the Tempo field shows (R6, R25).
    #[must_use]
    pub fn followed_tempo(&self) -> Option<f64> {
        if !self.tempo_follow_on {
            return None;
        }
        self.audio
            .tempo_tracking()
            .and_then(|tracking| tracking.followed())
    }

    /// Switches Follow, which gives the engine the tracker or takes it away.
    /// Switching it on switches EXT off and leaves it off (R5).
    fn switch_tempo_follower(&mut self, on: bool) {
        if on == self.tempo_follow_on {
            return;
        }
        self.tempo_follow_on = on;
        if on && self.midi.external_sync() {
            self.midi.set_external_sync(false);
            self.prefs.midi = self.midi.prefs();
            self.save_preferences();
            self.send_sync();
        }
        self.start_tempo_follower();
        self.touch();
    }

    /// Gives the engine a tracker for the chosen channel while Follow is on,
    /// or takes the tracker away while it's off.
    fn start_tempo_follower(&mut self) {
        let follow = self
            .tempo_follow_on
            .then(|| (self.tempo_follower_input(), crate::transport::tempo(self)));
        self.audio.set_tempo_follower(follow);
    }

    /// Keeps Follow honest and takes what the engine played into the set:
    /// Follow goes off when its input can't be reached (R4), and the
    /// followed tempo becomes the set's tempo, with no undo step (R6).
    /// Returns whether anything the UI shows changed.
    pub(super) fn track_tempo_follower(&mut self, now: Instant) -> bool {
        if !self.tempo_follow_on {
            return false;
        }
        if self.tempo_follower_unavailable().is_some() {
            self.switch_tempo_follower(false);
            return true;
        }
        let Some(bpm) = self
            .audio
            .tempo_tracking_mut()
            .and_then(|tracking| tracking.hear(now))
        else {
            return false;
        };
        self.follow_session_tempo(bpm)
    }

    /// Runs the `tempo_follower.*` commands and the two settings.
    pub(super) fn run_tempo_follower(
        &mut self,
        id: &str,
        args: Value,
    ) -> Result<Value, ShellError> {
        match id {
            ids::STATE => {
                Self::args::<NoArgs>(id, args)?;
                Ok(to_value(&self.tempo_follower_state()))
            }
            ids::SET_ENABLED => {
                let ids::FollowArgs { on } = Self::args(id, args)?;
                let on = on.unwrap_or(!self.tempo_follower_on());
                if on && !self.tempo_follower_toggle_shown() {
                    return Err(Self::refused(
                        id,
                        "Show Tempo Follower Toggle is Hide: Tempo Follower never runs while the \
                         toggle is hidden"
                            .to_owned(),
                    ));
                }
                if on && let Some(reason) = self.tempo_follower_unavailable() {
                    return Err(Self::refused(id, reason));
                }
                self.switch_tempo_follower(on);
                Ok(to_value(&ids::FollowSwitched { on }))
            }
            ids::SET_SHOW_TOGGLE => {
                let ids::FollowArgs { on } = Self::args(id, args)?;
                let on = on.unwrap_or(!self.tempo_follower_toggle_shown());
                self.prefs.tempo_follower.show_toggle = on;
                if !on {
                    self.switch_tempo_follower(false);
                }
                self.save_preferences();
                self.touch();
                Ok(to_value(&ids::FollowSwitched { on }))
            }
            _ => {
                let ids::SetInputArgs { channel } = Self::args(id, args)?;
                self.prefs.tempo_follower.input = channel;
                if self.tempo_follow_on {
                    if self.tempo_follower_unavailable().is_some() {
                        self.switch_tempo_follower(false);
                    } else {
                        self.start_tempo_follower();
                    }
                }
                self.save_preferences();
                self.touch();
                Ok(to_value(&ids::InputSet {
                    label: channel.label(),
                    channel,
                }))
            }
        }
    }

    fn refused(command: &str, reason: String) -> ShellError {
        ShellError::Unavailable {
            command: command.to_owned(),
            reason,
        }
    }

    /// What `tempo_follower.state` reports.
    fn tempo_follower_state(&mut self) -> ids::TempoFollowerState {
        let unavailable = self.tempo_follower_unavailable();
        let input = self.tempo_follower_input();
        let on = self.tempo_follow_on;
        let followed = self.followed_tempo();
        let tracking = self
            .audio
            .tempo_tracking()
            .map(|t| (t.estimate(), t.dropped()));
        ids::TempoFollowerState {
            on,
            show_toggle: self.tempo_follower_toggle_shown(),
            input,
            input_label: input.label(),
            can_run: unavailable.is_none(),
            unavailable,
            tempo: followed,
            confidence: tracking.map_or(0.0, |(estimate, _)| estimate.confidence),
            phase: tracking.map_or(0.0, |(estimate, _)| estimate.phase),
            blocks_dropped: tracking.map_or(0, |(_, dropped)| dropped),
        }
    }
}
