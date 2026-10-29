//! The transport as the app shell shows it (WP-1.18): the query that reads
//! what the Control Bar's transport section shows, and the IDs of the
//! transport's commands the menus and the Control Bar run.
//!
//! The transport's own commands (play, stop, locate, and the rest) are
//! `bl-engine`'s (`bl_engine::transport`), and the loop's are the
//! document's (`bl_model::registry`); `bl-shell` registers and runs them
//! all. This crate holds only their IDs, as data, so the menus can name
//! them without linking the engine. The rules are in
//! the control bar transport spec.

use bl_commands::Context::Global;
use bl_commands::{CommandSpec, NoArgs, Permission, RegisterError, Registry};
use bl_core::time::Ticks;
use bl_model::{MetronomeSettings, TrackId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `transport.state`: what the transport and the Control Bar's transport
/// section show.
pub const STATE: &str = "transport.state";

/// `transport.play` (`bl-engine`): the Control Bar's Play button.
pub const PLAY: &str = "transport.play";
/// `transport.stop` (`bl-engine`): the Control Bar's Stop button.
pub const STOP: &str = "transport.stop";
/// `transport.play_stop` (`bl-engine`): the space bar, and the Playback
/// menu's Play item, which reads Stop while the song plays.
pub const PLAY_STOP: &str = "transport.play_stop";
/// `transport.locate` (`bl-engine`): the Arrangement Position fields.
pub const LOCATE: &str = "transport.locate";
/// `transport.return_to_start` (`bl-engine`): the Playback menu's Return
/// Play Position to 1.1.1.
pub const RETURN_TO_START: &str = "transport.return_to_start";
/// `transport.set_loop` (`bl-model`): the Loop Start and Loop Length
/// fields.
pub const SET_LOOP: &str = "transport.set_loop";
/// `transport.set_loop_enabled` (`bl-model`): the Arrangement Loop switch.
pub const SET_LOOP_ENABLED: &str = "transport.set_loop_enabled";
/// `transport.set_punch_in` (`bl-model`): the Punch-In switch.
pub const SET_PUNCH_IN: &str = "transport.set_punch_in";
/// `transport.set_punch_out` (`bl-model`): the Punch-Out switch.
pub const SET_PUNCH_OUT: &str = "transport.set_punch_out";
/// `transport.set_metronome_enabled` (`bl-model`): the Metronome switch.
pub const SET_METRONOME_ENABLED: &str = "transport.set_metronome_enabled";
/// `transport.set_metronome_count_in` (`bl-model`): Count-In in Metronome Settings.
pub const SET_METRONOME_COUNT_IN: &str = "transport.set_metronome_count_in";
/// `transport.set_metronome_sound` (`bl-model`): Sound in Metronome Settings.
pub const SET_METRONOME_SOUND: &str = "transport.set_metronome_sound";
/// `transport.set_metronome_rhythm` (`bl-model`): Rhythm in Metronome Settings.
pub const SET_METRONOME_RHYTHM: &str = "transport.set_metronome_rhythm";
/// `transport.set_metronome_only_while_recording` (`bl-model`): Hear Metronome Only While Recording.
pub const SET_METRONOME_ONLY_WHILE_RECORDING: &str = "transport.set_metronome_only_while_recording";
/// `transport.set_midi_overdub` (`bl-model`): Options ▸ MIDI Arrangement Overdub.
pub const SET_MIDI_OVERDUB: &str = "transport.set_midi_overdub";
/// `transport.set_recording` (`bl-engine`): whether transport is recording.
pub const SET_RECORDING: &str = "transport.set_recording";
/// `transport.tap_tempo`: the Control Bar's Tap Tempo button (the control bar
/// transport spec, R18 to R22).
pub const TAP_TEMPO: &str = "transport.tap_tempo";
/// `song.set_tempo` (`bl-model`): the Tempo field.
pub const SET_TEMPO: &str = "song.set_tempo";
/// `song.set_time_signatures` (`bl-model`): the time-signature fields.
pub const SET_TIME_SIGNATURES: &str = "song.set_time_signatures";

/// Every command this module registers.
pub const IDS: &[&str] = &[STATE, TAP_TEMPO];

/// Whether `id` is one of the commands [`register`] adds.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// What `transport.state` returns: the transport and the Control Bar's
/// transport section, as they are now.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TransportState {
    /// Whether the audio engine runs on a device, so the transport can
    /// play. While it doesn't, the transport's commands are refused.
    pub engine_running: bool,
    /// Whether the song is playing.
    pub playing: bool,
    /// The play position (the playhead), in ticks from 1.1.1 (705,600 to
    /// the quarter note): where the song plays now, or where playback
    /// stopped, which is where `transport.continue` resumes.
    pub position: Ticks,
    /// The insert marker, where `transport.play` starts, in ticks.
    pub insert_marker: Ticks,
    /// What the Control Bar's Arrangement Position fields show, as
    /// bar.beat.sixteenth by the song's time signatures: the play position
    /// while the song plays, and the insert marker while it's stopped.
    pub arrangement_position: String,
    /// Where the Arrangement loop brace starts, in ticks.
    pub loop_start: Ticks,
    /// How long the loop brace is, in ticks.
    pub loop_length: Ticks,
    /// Whether the Arrangement Loop switch is on.
    pub loop_on: bool,
    /// The loop brace's start as the Loop Start fields show it, as
    /// bar.beat.sixteenth.
    pub loop_start_position: String,
    /// The brace's length as the Loop Length fields show it: bars, beats,
    /// and sixteenths counted from 0, such as `4.0.0` for four bars.
    pub loop_length_bars: String,
    /// The tempo where the Arrangement Position fields are, in quarter
    /// notes per minute, as the Tempo field shows it.
    pub tempo: f64,
    /// Whether the tempo is one constant tempo, so the Tempo field can set
    /// it. With tempo changes along the song, change it with
    /// `song.set_tempo`'s full map instead.
    pub tempo_editable: bool,
    /// The time signature there, such as `4/4`, as the time-signature fields
    /// show it.
    pub time_signature: String,
    /// Whether the song has one time signature throughout, so the
    /// time-signature fields can set it.
    pub time_signature_editable: bool,
    /// The metronome settings: enabled, count-in, sound, rhythm, and only
    /// while recording (WP-2.15).
    pub metronome: MetronomeSettings,
    /// Whether Arrangement Record is on, as the Control Bar's record button
    /// shows it. While it is on and the song plays, armed tracks record.
    pub arrangement_record: bool,
    /// Whether a take is running: Arrangement Record is on and the armed
    /// tracks' input is being written, so playback is past the count-in
    /// and any punch-in point. It ends at the punch-out point or when
    /// recording does.
    pub take_running: bool,
    /// Whether the count-in is sounding, before playback and recording
    /// start.
    pub counting_in: bool,
    /// Whether Punch-In is on.
    pub punch_in: bool,
    /// Whether Punch-Out is on.
    pub punch_out: bool,
    /// Whether MIDI Arrangement Overdub is on.
    pub midi_overdub: bool,
    /// Whether Automation Arm is on (view state, not saved with the set).
    pub automation_arm: bool,
    /// The mode an Arrangement recording writes control moves in, as
    /// `settings.set_automation_record_mode` sets it (the automation spec,
    /// R18).
    pub automation_record_mode: crate::settings::AutomationRecordMode,
    /// The recording under way, from the moment recording starts until its
    /// clips are made; null when none is.
    pub recording: Option<RunningRecording>,
    /// What the last recording that ended came to; null until one has
    /// ended in this session.
    pub last_recording: Option<RecordingReport>,
    /// Whether Capture MIDI has notes to bring back: a monitoring MIDI
    /// track was played since the last capture or recording, so `midi.capture`
    /// runs and the Control Bar's button and Create menu item are enabled.
    #[serde(default)]
    pub capture_midi_ready: bool,
    /// The run of taps Tap Tempo has now (the control bar transport spec,
    /// R22).
    #[serde(default)]
    pub tap_tempo: TapTempo,
}

/// Arguments of `transport.tap_tempo`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TapTempoArgs {
    /// The tap's frame on the engine clock; left out, the tap is now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<u64>,
}

/// A run of taps, as `transport.tap_tempo` returns it and `transport.state`
/// reports it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TapTempo {
    /// How many taps the run has; 0 once a new run would start.
    pub taps: u32,
    /// The tempo the run set, in quarter notes per minute: none until its
    /// second tap.
    pub tempo: Option<f64>,
    /// The engine-clock frame a tap start waits for, if one does.
    pub start_at: Option<u64>,
}

/// The recording under way, in `transport.state`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RunningRecording {
    /// The armed tracks it records, audio and MIDI.
    pub armed_tracks: Vec<TrackId>,
    /// Frames and MIDI messages dropped so far because the disk or the main
    /// thread fell behind.
    pub dropped_frames: u64,
    /// The Session slots it records into (WP-3.11); empty for an
    /// Arrangement recording.
    #[serde(default)]
    pub session_slots: Vec<crate::session_record::RecordingSlot>,
}

/// A file a recording wrote.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RecordedFileReport {
    /// The track it holds.
    pub track: TrackId,
    /// The file's path.
    pub path: String,
    /// The frames written.
    pub frames: u64,
    /// Its channels.
    pub channels: usize,
}

/// What the last recording that ended came to, in `transport.state`: the
/// same facts the Status Bar showed when it ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RecordingReport {
    /// Why it failed, if it did: it made no clips. Null when it made its
    /// clips, or when nothing was played.
    pub error: Option<String>,
    /// Whether it made clips in the set.
    pub clips_made: bool,
    /// The audio files it wrote, one per armed audio track that was played.
    pub files: Vec<RecordedFileReport>,
    /// The MIDI tracks that received notes or controllers.
    pub midi_tracks: Vec<TrackId>,
    /// Frames and MIDI messages dropped: the recording ended early.
    pub dropped_frames: u64,
    /// Frames the audio was moved earlier to line up with what was played.
    pub delay_frames: i64,
}

/// Adds the transport query to a registry.
///
/// # Errors
///
/// [`RegisterError`] if its ID is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    registry.register(
        CommandSpec::new(TAP_TEMPO, "Tap Tempo")
            .description(
                "Taps the tempo, as the Control Bar's Tap Tempo button does: each tap is one beat \
                 of the time signature, and from the second tap of a run every tap sets the \
                 song's tempo to the mean of the run's last eight intervals or fewer, to the \
                 hundredth within 20 to 999 BPM, as one undo step named Tap Tempo. A tap more \
                 than 3 seconds after the last starts a new run, and one less than 60 ms after \
                 it is ignored. `at` is the tap's frame on the engine clock, \
                 which counts the frames the engine has processed; left out, the tap is now. With Start Transport With Tap Tempo on \
                 and the song stopped, the tap that makes the run as many taps as the time \
                 signature's numerator starts the song one beat later, on a frame the result \
                 gives as `start_at`; Stop before it cancels the start. Refused while the song \
                 has tempo changes. The result gives the run's `taps`, its `tempo` and \
                 `start_at`; `transport.state` reports them under `tap_tempo`.",
            )
            .permission(Permission::Edit)
            .context(Global)
            .args::<TapTempoArgs>()
            .result::<TapTempo>(),
    )?;
    registry.register(
        CommandSpec::new(STATE, "Transport Status")
            .description(
                "Reports what the Control Bar's transport section shows: whether the song \
                 plays, the play position and the insert marker (in ticks from 1.1.1, and the \
                 Arrangement Position fields' bar.beat.sixteenth), the Arrangement loop's start, \
                 length, and switch, the tempo, and the time signature, the metronome, \
                 Arrangement Record and whether a take is running or the count-in is \
                 sounding, Punch-In and Punch-Out, MIDI Arrangement Overdub, Automation Arm and \
                 the mode an Arrangement recording writes control moves in, \
                 the recording under way (its armed tracks and dropped frames) and what the \
                 last recording came to, and whether the audio engine runs so the transport \
                 can play. Every `transport.set_*` setting reads back here. Changes nothing. Use it to check where \
                 the song is before and after `transport.play`, `transport.stop`, \
                 `transport.locate`, or `transport.set_loop`.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<TransportState>(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_query_registers_complete() {
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        assert_eq!(registry.len(), IDS.len());
        assert!(registry.problems().is_empty(), "{:?}", registry.problems());
        assert!(handles(STATE));
        assert!(!handles(PLAY), "the engine's");
    }
}
