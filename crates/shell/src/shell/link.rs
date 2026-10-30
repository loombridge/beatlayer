//! Link: switching it, its state, and a peer's tempo in the set (the Link
//! spec, R1, R3, R8, R14, R15).

use std::time::Instant;

use super::{Shell, ShellError, to_value};
use crate::settings::OnByDefault;
use bl_commands::NoArgs;
use bl_shell_spec::link as ids;
use serde_json::Value;

impl Shell {
    /// Whether Link is on.
    #[must_use]
    pub fn link_enabled(&self) -> bool {
        self.link.is_enabled()
    }

    /// How many other peers are in the Link session; none while Link is off.
    #[must_use]
    pub fn link_peers(&self) -> u64 {
        self.link.peers()
    }

    /// Whether Start Stop Sync is on.
    #[must_use]
    pub fn link_start_stop_sync(&self) -> bool {
        self.prefs.link.start_stop_sync
    }

    /// Whether the Control Bar shows the Link toggle and its menu (the Link
    /// spec, R17).
    #[must_use]
    pub fn link_toggle_shown(&self) -> bool {
        self.prefs.link.show_toggle.0
    }

    /// How far the session is through the quantum, from 0 to 1, while the
    /// Arrangement Position fields show the quantum's progress bar: with a
    /// peer, and the transport stopped or waiting for a start (the Link
    /// spec, R22, R25). Read from what Link and the engine last published.
    #[must_use]
    pub fn link_progress(&self) -> Option<f64> {
        if !self.link_leads() || (self.playing() && !self.link_waiting_view) {
            return None;
        }
        let quantum = self.link_quantum();
        let session = self.link.snapshot(quantum)?;
        Some((session.phase / quantum).clamp(0.0, 1.0))
    }

    /// Whether a peer's change of the tempo took the set's tempo envelope's
    /// place, which lights Re-Enable Automation (the Link spec, R4).
    #[must_use]
    pub fn tempo_overridden(&self) -> bool {
        self.tempo_override
    }

    /// The session's tempo, while a peer's change took the tempo envelope's
    /// place: what the Tempo field shows then.
    #[must_use]
    pub fn link_override_tempo(&self) -> Option<f64> {
        if !self.tempo_override {
            return None;
        }
        self.link.snapshot(1.0).map(|session| session.tempo)
    }

    /// Whether Link is on and has a peer: the song follows the session's
    /// tempo and a start waits for its phase.
    #[must_use]
    pub fn link_leads(&self) -> bool {
        self.link.peers() > 0
    }

    /// Switches Link on at startup if the preferences keep it on (the Link
    /// spec, R1).
    pub(super) fn start_link(&mut self) {
        if self.prefs.link.enabled {
            self.switch_link(true);
        }
    }

    /// Joins or leaves the Link session, which a new session starts at the
    /// set's tempo (R2).
    fn switch_link(&mut self, on: bool) {
        let bpm = crate::transport::tempo(self);
        if on && self.link.make(bpm) {
            self.audio.set_link(self.link.handle());
        }
        self.link.enable(on, bpm);
        self.link
            .set_start_stop_sync(self.prefs.link.start_stop_sync);
    }

    /// Runs the `link.*` commands.
    pub(super) fn run_link(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        match id {
            ids::STATE => {
                Self::args::<NoArgs>(id, args)?;
                let engine = self.poll_engine();
                Ok(to_value(&self.link_state(&engine)))
            }
            ids::SET_ENABLED => {
                let ids::SetArgs { on } = Self::args(id, args)?;
                let on = on.unwrap_or(!self.link_enabled());
                self.prefs.link.enabled = on;
                self.switch_link(on);
                self.save_preferences();
                self.touch();
                Ok(to_value(&ids::Switched { on }))
            }
            ids::SET_SHOW_TOGGLE => {
                let ids::SetArgs { on } = Self::args(id, args)?;
                let on = on.unwrap_or(!self.link_toggle_shown());
                self.prefs.link.show_toggle = OnByDefault(on);
                self.save_preferences();
                self.touch();
                Ok(to_value(&ids::Switched { on }))
            }
            _ => {
                let ids::SetArgs { on } = Self::args(id, args)?;
                let on = on.unwrap_or(!self.prefs.link.start_stop_sync);
                self.prefs.link.start_stop_sync = on;
                self.link.set_start_stop_sync(on);
                self.save_preferences();
                self.touch();
                Ok(to_value(&ids::Switched { on }))
            }
        }
    }

    /// What `link.state` reports.
    fn link_state(&self, engine: &crate::audio::EngineTransport) -> ids::LinkState {
        let quantum = self.link_quantum();
        let session = self.link.snapshot(quantum);
        let waiting_for_link = self.link_waiting(engine);
        let starts_in = engine
            .waiting_frame
            .filter(|_| waiting_for_link)
            .map(|frame| {
                frame.saturating_sub(engine.clock) as f64
                    / f64::from(self.audio.sample_rate().unwrap_or(48_000))
            });
        ids::LinkState {
            enabled: self.link_enabled(),
            peers: self.link.peers(),
            tempo: session.map(|session| session.tempo),
            quantum,
            phase: session.map(|session| session.phase),
            session_playing: session.is_some_and(|session| session.playing),
            start_stop_sync: self.prefs.link.start_stop_sync,
            show_toggle: self.link_toggle_shown(),
            waiting_for_link,
            starts_in,
        }
    }

    /// Whether a start waits for the Link session's phase.
    pub(crate) fn link_waiting(&self, engine: &crate::audio::EngineTransport) -> bool {
        engine.waiting_start && self.link_enabled() && self.link_leads()
    }

    /// The global launch quantisation's length at the play position, in
    /// quarter notes (the Link spec, R5).
    fn link_quantum(&self) -> f64 {
        let at = self.arrangement.transport().position;
        self.launch_settings()
            .quantum_phase(at, &self.editor.document().settings().time_signatures)
            .0
            .as_quarters()
    }

    /// Takes what Link told the main thread: a peer's tempo becomes the
    /// set's, at most once every 100 ms while it moves, with no undo step
    /// (R3). Returns whether anything the UI shows changed.
    pub(super) fn track_link(&mut self, now: Instant) -> bool {
        let peers = self.link.peers();
        let mut changed = std::mem::replace(&mut self.link_peers_shown, peers) != peers;
        if let Some(bpm) = self.link.hear(now) {
            changed |= self.follow_session_tempo(bpm);
        }
        changed
    }

    /// Writes a peer's tempo into the set when the set has one constant
    /// tempo. A tempo envelope stays as it is: the engine plays the
    /// session's tempo over it, until Re-Enable Automation (R4).
    fn follow_session_tempo(&mut self, bpm: f64) -> bool {
        if !self.write_session_tempo(bpm) {
            return false;
        }
        self.document_changed();
        self.touch();
        true
    }

    /// Makes the set's constant tempo `bpm`, with no undo step, and marks
    /// the set changed. Returns whether it changed.
    fn write_session_tempo(&mut self, bpm: f64) -> bool {
        let changes = {
            let doc = self.editor.document();
            let settings = doc.settings();
            let mut segments = settings.tempo.segments();
            let constant = segments.len() == 1
                && segments
                    .next()
                    .is_some_and(|segment| segment.start_bpm() == segment.end_bpm());
            constant
                && !doc.any_tempo_leader()
                && (settings.tempo.bpm_at(bl_core::time::Ticks::ZERO).get() - bpm).abs() >= 5e-4
        };
        let Ok(bpm) = bl_core::time::Bpm::new(bpm) else {
            return false;
        };
        if !changes {
            return false;
        }
        self.editor.follow_tempo(bpm);
        self.dirty = true;
        true
    }

    /// A set opened or made while the session has peers keeps the session's
    /// tempo (the Link spec, R2): a set with one tempo takes it, before the
    /// engine hears the set, so the set's tempo isn't taken for a change of
    /// Beatlayer's own.
    pub(super) fn keep_session_tempo(&mut self) {
        if self.link_enabled()
            && self.link_leads()
            && let Some(session) = self.link.snapshot(1.0)
        {
            self.write_session_tempo(session.tempo);
        }
    }
}
