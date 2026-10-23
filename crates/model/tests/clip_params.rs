//! An audio clip's sound settings as parameters (WP-4.8, the warp modes spec,
//! R28 to R32): every one has a descriptor and can be automated on the track
//! and drawn as a clip envelope, by the commands' IDs, one undo step each.

use bl_commands::validate;
use bl_core::time::Ticks;
use bl_model::param::MINUS_INFINITY_DB;
use bl_model::registry;
use bl_model::{
    AudioData, AutomationTarget, Clip, ClipData, ClipId, ClipParam, Command, ContentHash, Editor,
    EnvelopeKind, MidiData, TrackId, TrackKind,
};
use serde_json::{Value, json};

/// A set with an audio track holding an audio clip, and a MIDI track holding
/// a MIDI clip.
struct Song {
    editor: Editor,
    audio_track: TrackId,
    audio: ClipId,
    midi_track: TrackId,
    midi: ClipId,
}

impl Song {
    fn new() -> Self {
        let mut editor = Editor::new();
        let mut track = |kind| {
            let id = editor.new_id();
            editor
                .apply(Command::CreateTrack {
                    id,
                    kind,
                    name: String::new(),
                    parent: None,
                    index: 0,
                })
                .unwrap();
            id
        };
        let (audio_track, midi_track) = (track(TrackKind::Audio), track(TrackKind::Midi));
        let (audio, midi) = (editor.new_id(), editor.new_id());
        let audio_clip = Clip::new(
            audio,
            Ticks::ZERO,
            Ticks::WHOLE,
            ClipData::Audio(AudioData::new(ContentHash([1; 32]))),
        );
        let midi_clip = Clip::new(
            midi,
            Ticks::ZERO,
            Ticks::WHOLE,
            ClipData::Midi(MidiData::new([])),
        );
        for (track, clip) in [(audio_track, audio_clip), (midi_track, midi_clip)] {
            editor.apply(Command::CreateClip { track, clip }).unwrap();
        }
        Self {
            editor,
            audio_track,
            audio,
            midi_track,
            midi,
        }
    }

    /// Runs a command by ID as an agent does: the arguments checked against
    /// the schema, and the run one undo step.
    fn run(&mut self, id: &str, args: Value) -> Result<(), String> {
        let spec_args = {
            let mut commands = bl_commands::Registry::new();
            registry::register(&mut commands).unwrap();
            commands.get(id).unwrap().args.clone().unwrap()
        };
        validate(&spec_args, &args).unwrap_or_else(|e| panic!("{id}: {e}"));
        let before = self.editor.undo_history().len();
        registry::run(&mut self.editor, id, args).map_err(|e| e.to_string())?;
        assert_eq!(
            self.editor.undo_history().len(),
            before + 1,
            "{id} is one undo step"
        );
        Ok(())
    }

    fn automation(&self, param: ClipParam) -> Option<(bool, Vec<(i64, f64)>)> {
        let track = self.editor.document().track(self.audio_track).unwrap();
        track
            .automation()
            .find(|(target, _)| **target == AutomationTarget::Clip { param })
            .map(|(_, automation)| {
                (
                    automation.enabled,
                    automation
                        .envelope
                        .points()
                        .map(|p| (p.at.get(), p.value))
                        .collect(),
                )
            })
    }

    fn envelope(&self, param: ClipParam) -> Option<Vec<(i64, f64)>> {
        self.editor
            .document()
            .clip(self.audio)
            .unwrap()
            .envelope(&AutomationTarget::Clip { param })
            .map(|e| e.envelope.points().map(|p| (p.at.get(), p.value)).collect())
    }
}

fn target(param: ClipParam) -> Value {
    json!({ "type": "clip", "param": param })
}

// Spec: warp-modes R28 (rules G1 and G2)
#[test]
fn every_clip_sound_setting_is_an_automation_and_clip_envelope_target() {
    let mut song = Song::new();
    for (index, param) in ClipParam::ALL.into_iter().enumerate() {
        let who = format!("{param:?}");
        assert_eq!(param as usize, index, "{who}: ALL is in the enum's order");
        let descriptor = param.descriptor();
        descriptor.check().unwrap_or_else(|e| panic!("{who}: {e}"));
        // What the engine reads without building the descriptor agrees with
        // it.
        assert_eq!(
            param.limits(),
            (descriptor.min(), descriptor.max()),
            "{who}"
        );
        assert_eq!(param.is_stepped(), descriptor.is_stepped(), "{who}");
        assert!(descriptor.automatable, "{who}");
        // The descriptor's own range is the automation's: its ends are taken,
        // and a value past either is refused.
        let mut ends = vec![descriptor.min(), descriptor.default, descriptor.max()];
        ends.dedup();
        for value in ends {
            song.run(
                "automation.set_breakpoint",
                json!({ "track": song.audio_track, "target": target(param),
                        "at": 0, "value": value }),
            )
            .unwrap_or_else(|e| panic!("{who} automation at {value}: {e}"));
        }
        let too_far = descriptor.max() + 1.0;
        assert!(
            song.editor
                .apply(Command::SetAutomationBreakpoint {
                    track: song.audio_track,
                    target: AutomationTarget::Clip { param },
                    at: Ticks::ZERO,
                    value: too_far,
                })
                .is_err(),
            "{who}: automation past its range"
        );
        // The clip envelope's range is R29's.
        let (low, high) = param.envelope_range();
        for value in [low, high] {
            song.run(
                "clip.set_envelope_breakpoint",
                json!({ "clip": song.audio, "target": target(param),
                        "at": 0, "value": value }),
            )
            .unwrap_or_else(|e| panic!("{who} envelope at {value}: {e}"));
        }
        assert!(
            song.editor
                .apply(Command::SetClipEnvelopeBreakpoint {
                    clip: song.audio,
                    target: AutomationTarget::Clip { param },
                    kind: EnvelopeKind::Modulation,
                    at: Ticks::ZERO,
                    value: high + 1.0,
                })
                .is_err(),
            "{who}: envelope past its range"
        );
        assert_eq!(song.envelope(param), Some(vec![(0, high)]), "{who}");
    }
    // G1: the clip's Gain reaches −∞ dB, in the automation and in the clip
    // envelope's share.
    let gain = ClipParam::Gain.descriptor();
    assert!(gain.is_gain());
    assert_eq!(gain.min(), 0.0);
    assert_eq!(gain.format(0.0), MINUS_INFINITY_DB);
    assert_eq!(ClipParam::Gain.moved(1.0, Some(0.0)), 0.0);
    assert_eq!(
        song.automation(ClipParam::Gain).unwrap().1,
        [(0, gain.max())]
    );
}

// Spec: warp-modes R29. Gain's envelope is a share of the clip's Gain, so it
// never takes the clip above it; the rest add to their setting and are held
// to its range.
#[test]
fn clip_envelopes_move_a_setting_inside_its_range() {
    assert_eq!(ClipParam::Gain.moved(0.5, Some(100.0)), 0.5);
    assert_eq!(ClipParam::Gain.moved(0.5, Some(50.0)), 0.25);
    assert_eq!(ClipParam::Gain.moved(0.5, None), 0.5);
    assert_eq!(ClipParam::Transpose.moved(40.0, Some(12.0)), 48.0);
    assert_eq!(ClipParam::Transpose.moved(3.0, Some(-7.5)), -4.5);
    assert_eq!(ClipParam::Detune.moved(30.0, Some(40.0)), 50.0);
    assert_eq!(ClipParam::Formants.moved(80.0, Some(-30.0)), 50.0);
    // A choice or a switch takes the step nearest the sum.
    assert_eq!(ClipParam::Preserve.moved(1.0, Some(1.4)), 2.0);
    assert_eq!(ClipParam::HighQuality.moved(0.0, Some(0.7)), 1.0);
    assert_eq!(ClipParam::SampleOffset.moved(0.0, Some(3.5)), 3.5);
}

// Spec: warp-modes R28, R31
#[test]
fn only_an_audio_track_and_an_audio_clip_take_a_clip_setting_target() {
    let mut song = Song::new();
    let setting = AutomationTarget::Clip {
        param: ClipParam::Detune,
    };
    assert!(
        song.editor
            .apply(Command::SetAutomationBreakpoint {
                track: song.midi_track,
                target: setting,
                at: Ticks::ZERO,
                value: 0.0,
            })
            .is_err()
    );
    for clip in [song.midi, song.audio] {
        let result = song.editor.apply(Command::SetClipEnvelopeBreakpoint {
            clip,
            target: setting,
            kind: EnvelopeKind::Modulation,
            at: Ticks::ZERO,
            value: 0.0,
        });
        assert_eq!(result.is_ok(), clip == song.audio);
    }
    // Only Session clips have an Automation envelope, and not for a setting.
    assert!(
        song.editor
            .apply(Command::SetClipEnvelopeLinked {
                clip: song.audio,
                target: setting,
                kind: EnvelopeKind::Automation,
                linked: true,
            })
            .is_err()
    );
}

// Spec: warp-modes R31. A hand move of a setting turns the track's automation
// of it off, until Re-Enable Automation.
#[test]
fn a_hand_move_of_a_setting_overrides_its_automation() {
    let mut song = Song::new();
    for param in [ClipParam::Gain, ClipParam::Transpose, ClipParam::Formants] {
        song.run(
            "automation.set_breakpoint",
            json!({ "track": song.audio_track, "target": target(param),
                    "at": 0, "value": param.descriptor().default }),
        )
        .unwrap();
    }
    song.editor
        .apply(Command::SetClipGain {
            clip: song.audio,
            gain: 0.5,
        })
        .unwrap();
    song.editor
        .apply(Command::SetClipPitch {
            clip: song.audio,
            transpose: Some(2),
            detune: None,
        })
        .unwrap();
    assert!(!song.automation(ClipParam::Gain).unwrap().0, "Gain");
    assert!(
        !song.automation(ClipParam::Transpose).unwrap().0,
        "Transpose"
    );
    assert!(song.automation(ClipParam::Formants).unwrap().0, "untouched");
    song.run("automation.re_enable", json!({})).unwrap();
    assert!(song.automation(ClipParam::Gain).unwrap().0);
    assert!(song.automation(ClipParam::Transpose).unwrap().0);
}

// Spec: warp-modes R32
#[test]
fn clip_targets_take_automation_and_envelope_commands() {
    let mut song = Song::new();
    let formants = ClipParam::Formants;
    for (id, args) in [
        (
            "automation.set_breakpoint",
            json!({ "track": song.audio_track, "target": target(formants), "at": 0, "value": 40.0 }),
        ),
        (
            "automation.set_breakpoint",
            json!({ "track": song.audio_track, "target": target(formants), "at": 960, "value": 90.0 }),
        ),
        (
            "automation.set_curve",
            json!({ "track": song.audio_track, "target": target(formants), "at": 0, "bend": 0.5 }),
        ),
        (
            "clip.set_envelope_breakpoint",
            json!({ "clip": song.audio, "target": target(formants), "at": 0, "value": -20.0 }),
        ),
        (
            "clip.set_envelope_breakpoint",
            json!({ "clip": song.audio, "target": target(formants), "at": 480, "value": 10.0 }),
        ),
        (
            "clip.move_envelope_breakpoint",
            json!({ "clip": song.audio, "target": target(formants), "at": 480, "to": 240 }),
        ),
        (
            "clip.set_envelope_linked",
            json!({ "clip": song.audio, "target": target(formants), "linked": false }),
        ),
    ] {
        song.run(id, args).unwrap_or_else(|e| panic!("{id}: {e}"));
    }
    assert_eq!(
        song.automation(formants),
        Some((true, vec![(0, 40.0), (960, 90.0)]))
    );
    assert_eq!(song.envelope(formants), Some(vec![(0, -20.0), (240, 10.0)]));
    song.run(
        "clip.delete_envelope_breakpoint",
        json!({ "clip": song.audio, "target": target(formants), "at": 240 }),
    )
    .unwrap();
    song.run(
        "clip.delete_envelope",
        json!({ "clip": song.audio, "target": target(formants) }),
    )
    .unwrap();
    assert_eq!(song.envelope(formants), None);
    song.run(
        "automation.delete",
        json!({ "track": song.audio_track, "target": target(formants) }),
    )
    .unwrap();
    assert_eq!(song.automation(formants), None);
}
