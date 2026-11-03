//! The audio device and the real-time session.

use super::{Invocation, Message, Shell, ShellError, to_value};
use crate::audio::AudioHost;
use crate::commands as ids;
use bl_shell_spec::export as export_ids;
use serde_json::Value;
use serde_json::json;

use crate::audio::ConfigureAudio;
use crate::audio::ToneSettings;
use crate::commands::SwitchResult;
use crate::commands::TestToneArgs;
use crate::commands::TestToneState;
use bl_commands::NoArgs;

impl Shell {
    /// Opens the audio device, if the engine is on. A failure shows in the
    /// Status Bar with a way to the Audio settings.
    pub fn start_audio(&mut self) {
        if let Err(error) = self.audio.start() {
            self.audio_failed(&error);
        }
        self.touch();
    }

    pub(super) fn audio_failed(&mut self, error: &str) {
        self.message = Some(Message {
            text: format!(
                "The audio device couldn't start: {error}. Click to open the Audio settings."
            ),
            error: true,
            action: Some(Invocation::new(
                ids::APP_SETTINGS,
                json!({ "page": "audio" }),
            )),
        });
    }

    /// The audio device.
    #[must_use]
    pub fn audio(&self) -> &AudioHost {
        &self.audio
    }

    /// The audio device, to sample its CPU load.
    pub fn audio_mut(&mut self) -> &mut AudioHost {
        &mut self.audio
    }

    /// The audio interface the In/Out choosers list: the open device's
    /// channels, or none while no stream is open.
    #[must_use]
    pub fn interface(&self) -> crate::routing::Interface {
        let (inputs, outputs) = self.audio.channels();
        // The extra audio outputs the set's devices offer past their main
        // one, which another track's Audio From lists (WP-5.45).
        let mut device_outputs = std::collections::BTreeMap::new();
        for track in self.editor.document().tracks() {
            for device in track.devices_deep() {
                let outputs = self.devices.outputs(&device.kind);
                if !outputs.is_empty() {
                    device_outputs.insert(device.id, outputs.to_vec());
                }
            }
        }
        crate::routing::Interface {
            inputs,
            outputs,
            midi_inputs: self.midi.input_names(),
            mpe_inputs: self.midi.mpe_input_names(),
            midi_outputs: self.midi.output_names(),
            device_outputs,
            link_audio: self
                .link_audio_peers()
                .into_iter()
                .map(|peer| crate::routing::LinkPeer {
                    name: peer.name,
                    channels: peer.channels,
                })
                .collect(),
        }
    }

    /// The engine's rate: the audio device's, or the rate set in the audio
    /// settings, or 48 kHz when neither is known (the export spec, R6).
    #[must_use]
    pub fn engine_rate(&self) -> u32 {
        self.audio
            .sample_rate()
            .or(self.audio.settings().sample_rate)
            .unwrap_or(crate::export::DEFAULT_ENGINE_RATE)
    }

    /// The state of the real-time render an export with external hardware
    /// runs: the app's dialog shows it, and its buttons set it (the
    /// external devices spec, R23).
    #[must_use]
    pub fn realtime_session(&self) -> crate::realtime::Session {
        self.realtime.clone()
    }

    /// What the real-time render is doing now (the external devices spec,
    /// R23).
    #[must_use]
    pub fn realtime_status(&self) -> export_ids::RealtimeStatus {
        let status = self.realtime.status();
        let phase = match status.phase {
            crate::realtime::Phase::Idle => export_ids::RealtimePhase::Idle,
            crate::realtime::Phase::Waiting => export_ids::RealtimePhase::Waiting {
                seconds: status.seconds,
            },
            crate::realtime::Phase::Rendering => export_ids::RealtimePhase::Rendering,
        };
        export_ids::RealtimeStatus {
            phase,
            progress: status.progress,
            attempts: status.attempts,
            auto_restart: status.auto_restart,
        }
    }

    /// Runs the real-time render's dialog's commands (the external devices
    /// spec, R23).
    pub(super) fn run_realtime(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        match id {
            export_ids::REALTIME_STATUS => Ok(to_value(&self.realtime_status())),
            export_ids::REALTIME_RESTART => {
                self.realtime.restart();
                Ok(to_value(&self.realtime_status()))
            }
            export_ids::REALTIME_SKIP => {
                self.realtime.skip();
                Ok(to_value(&self.realtime_status()))
            }
            export_ids::REALTIME_CANCEL => {
                self.realtime.cancel();
                Ok(to_value(&self.realtime_status()))
            }
            export_ids::REALTIME_AUTO_RESTART => {
                let args = Self::args::<export_ids::AutoRestartArgs>(id, args)?;
                self.realtime.set_auto_restart(args.auto_restart);
                Ok(to_value(&self.realtime_status()))
            }
            _ => Err(ShellError::UnknownCommand(id.to_owned())),
        }
    }

    /// Runs the `audio` commands.
    pub(super) fn run_audio(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let failed = |message: String| ShellError::Failed {
            command: id.to_owned(),
            message,
        };
        match id {
            ids::AUDIO_DEVICES => {
                Self::args::<NoArgs>(id, args)?;
                Ok(to_value(&self.audio.devices()))
            }
            ids::AUDIO_STATUS => {
                Self::args::<NoArgs>(id, args)?;
                Ok(to_value(&self.audio.status()))
            }
            ids::AUDIO_CONFIGURE => {
                let change: ConfigureAudio = Self::args(id, args)?;
                let result = self.audio.configure(&change);
                self.prefs.audio = self.audio.settings().clone();
                self.save_preferences();
                result.map_err(failed)?;
                if self.message.as_ref().is_some_and(|m| m.error) {
                    self.message = None;
                }
                Ok(to_value(&self.audio.status()))
            }
            ids::AUDIO_TOGGLE_ENGINE => {
                Self::args::<NoArgs>(id, args)?;
                let on = !self.audio.engine_on();
                match self.audio.set_engine(on) {
                    Ok(()) if on => self.message = None,
                    Ok(()) => {
                        self.message = Some(Message {
                            text: "The audio engine is off. Click to open the Audio settings."
                                .to_owned(),
                            error: false,
                            action: Some(Invocation::new(
                                ids::APP_SETTINGS,
                                json!({ "page": "audio" }),
                            )),
                        });
                    }
                    Err(error) => self.audio_failed(&error),
                }
                Ok(to_value(&SwitchResult { on }))
            }
            ids::AUDIO_SET_TEST_TONE => {
                let TestToneArgs {
                    on,
                    volume_db,
                    frequency_hz,
                } = Self::args(id, args)?;
                if volume_db.is_some() || frequency_hz.is_some() {
                    let current = self.audio.tone();
                    self.audio.set_tone(ToneSettings {
                        volume_db: volume_db.unwrap_or(current.volume_db),
                        frequency_hz: frequency_hz.unwrap_or(current.frequency_hz),
                    });
                    self.prefs.tone = self.audio.tone();
                    self.save_preferences();
                }
                if let Some(on) = on {
                    self.audio.set_test_tone(on);
                }
                let tone = self.audio.tone();
                Ok(to_value(&TestToneState {
                    on: self.audio.test_tone(),
                    volume_db: tone.volume_db,
                    frequency_hz: tone.frequency_hz,
                }))
            }
            _ => Err(ShellError::UnknownCommand(id.to_owned())),
        }
    }
}
