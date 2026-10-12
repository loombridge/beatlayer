//! The rules for a track's In/Out settings (the routing spec): which
//! inputs and outputs each kind of track takes. A routing may make a loop,
//! as Live allows (observed); the engine's compiler breaks it.

use std::collections::BTreeSet;

use crate::document::Document;
use crate::error::ModelError;
use crate::ids::TrackId;
use crate::track::{Input, Output, Routing, Track, TrackKind};

impl Document {
    /// Where a track's audio goes in the end: Main, a group or another
    /// audio track, or nowhere (Sends Only, Ext. Out). `None` for Main
    /// itself.
    #[must_use]
    pub fn output_of(&self, track: TrackId) -> Option<TrackId> {
        let t = self.track(track)?;
        match t.kind() {
            TrackKind::Main => None,
            TrackKind::Return => match t.routing.output {
                Output::Default | Output::Main => Some(self.main),
                Output::Track { track } => Some(track),
                Output::External | Output::SendsOnly => None,
            },
            _ => match t.routing.output {
                Output::Default => Some(self.parent(track).unwrap_or(self.main)),
                Output::Main => Some(self.main),
                Output::Track { track } => Some(track),
                Output::External | Output::SendsOnly => None,
            },
        }
    }

    /// Checks a track's In/Out settings against its kind and the tracks
    /// they name.
    pub(crate) fn check_routing(&self, id: TrackId) -> Result<(), ModelError> {
        let track = self.track_ref(id)?;
        check_input(self, track)?;
        check_output(self, track)?;
        if let Some(to) = track.routing.midi_output {
            if track.kind != TrackKind::Midi {
                return Err(wrong_kind(track, "a MIDI track"));
            }
            self.check_target(track, to, |kind| kind == TrackKind::Midi)?;
        }
        Ok(())
    }

    fn check_target(
        &self,
        track: &Track,
        to: TrackId,
        allowed: impl Fn(TrackKind) -> bool,
    ) -> Result<(), ModelError> {
        let target = self.track(to).ok_or(ModelError::NoSuchTrack(to))?;
        if to == track.id || !allowed(target.kind) {
            return Err(ModelError::CannotRoute {
                track: track.id,
                to,
            });
        }
        Ok(())
    }

    /// Puts every routing that names `gone` out of use, for a track that's
    /// deleted, as Live does (observed): outputs to it become Sends Only,
    /// inputs from it No Input, and MIDI outputs to it No Output.
    pub(crate) fn forget_routes_to(&mut self, gone: &BTreeSet<TrackId>) {
        let touched: Vec<TrackId> = self
            .tracks
            .iter()
            .filter(|(_, t)| names_any(&t.routing, gone))
            .map(|(&id, _)| id)
            .collect();
        for id in touched {
            if let Some(track) = self.tracks.get_mut(&id) {
                let routing = &mut crate::shared::Shared::make_mut(track).routing;
                if let Output::Track { track } = routing.output
                    && gone.contains(&track)
                {
                    routing.output = Output::SendsOnly;
                }
                if let Input::Track { track, .. } = routing.input
                    && gone.contains(&track)
                {
                    routing.input = Input::None;
                }
                if routing.midi_output.is_some_and(|to| gone.contains(&to)) {
                    routing.midi_output = None;
                }
            }
        }
    }

    /// Brings a moved track's output up to date with where it is now: a
    /// track going into a group that was routed to Main goes to the group
    /// (as Live does), and a track routed to its group goes to its new
    /// group or Main.
    pub(crate) fn settle_output(&mut self, id: TrackId, old_parent: Option<TrackId>) {
        let parent = self.parent(id);
        let Some(track) = self.tracks.get_mut(&id) else {
            return;
        };
        let output = track.routing.output;
        let settled = match output {
            Output::Main if parent.is_some() && old_parent.is_none() => Output::Default,
            Output::Track { track } if Some(track) == parent => Output::Default,
            Output::Track { track } if Some(track) == old_parent => Output::Default,
            other => other,
        };
        if settled != output {
            crate::shared::Shared::make_mut(track).routing.output = settled;
        }
    }

    /// `output` as it's stored: Main from a top-level track, or the group
    /// a track is in, is [`Output::Default`].
    pub(crate) fn normal_output(&self, id: TrackId, output: Output) -> Output {
        let parent = self.parent(id);
        match output {
            Output::Main if parent.is_none() => Output::Default,
            Output::Track { track } if Some(track) == parent => Output::Default,
            other => other,
        }
    }
}

fn names_any(routing: &Routing, gone: &BTreeSet<TrackId>) -> bool {
    matches!(routing.output, Output::Track { track } if gone.contains(&track))
        || matches!(routing.input, Input::Track { track, .. } if gone.contains(&track))
        || routing.midi_output.is_some_and(|to| gone.contains(&to))
}

fn wrong_kind(track: &Track, needs: &'static str) -> ModelError {
    ModelError::WrongTrackKind {
        track: track.id,
        kind: track.kind,
        needs,
    }
}

/// An audio track takes Ext. In, Resampling, another audio or group
/// track, a return, Main, or nothing; a MIDI track All Ins, the computer keyboard,
/// another MIDI track, or nothing; other kinds take no input.
fn check_input(doc: &Document, track: &Track) -> Result<(), ModelError> {
    let input = track.routing.input;
    match (track.kind, input) {
        (TrackKind::Audio | TrackKind::Midi, Input::None)
        | (TrackKind::Audio, Input::External | Input::Resampling)
        | (TrackKind::Midi, Input::AllIns | Input::ComputerKeyboard) => Ok(()),
        (TrackKind::Audio, Input::Track { track: from, .. }) => {
            doc.check_target(track, from, |kind| {
                matches!(
                    kind,
                    TrackKind::Audio | TrackKind::Group | TrackKind::Return | TrackKind::Main
                )
            })
        }
        (TrackKind::Midi, Input::Track { track: from, .. }) => {
            doc.check_target(track, from, |kind| kind == TrackKind::Midi)
        }
        (TrackKind::Audio, _) => Err(wrong_kind(track, "a MIDI track")),
        (TrackKind::Midi, _) => Err(wrong_kind(track, "an audio track")),
        (_, Input::None) => Ok(()),
        _ => Err(wrong_kind(track, "an audio or MIDI track")),
    }
}

/// Audio, group, and return tracks go to Main, the group they're in, an
/// audio track, their sends only, or Ext. Out; MIDI tracks (which have no
/// instrument yet) and Main where they go.
fn check_output(doc: &Document, track: &Track) -> Result<(), ModelError> {
    let output = track.routing.output;
    let routed = matches!(
        track.kind,
        TrackKind::Audio | TrackKind::Group | TrackKind::Return
    );
    match (track.kind, output) {
        (_, Output::Default) => Ok(()),
        (_, Output::Main | Output::External | Output::SendsOnly) if routed => Ok(()),
        (_, Output::Track { track: to }) if routed => {
            let parent = doc.parent(track.id);
            doc.check_target(track, to, |kind| kind == TrackKind::Audio)
                .or_else(|error| {
                    if Some(to) == parent {
                        Ok(())
                    } else {
                        Err(error)
                    }
                })
        }
        (TrackKind::Main, _) => Err(ModelError::MainTrack),
        _ => Err(wrong_kind(track, "an audio, group, or return track")),
    }
}

#[cfg(test)]
mod tests {
    use crate::command::Command;
    use crate::editor::Editor;
    use crate::error::ModelError;
    use crate::ids::TrackId;
    use crate::track::{Input, Monitor, Output, Routing, Tap, TrackKind};
    use crate::{Document, SendTap};

    fn add(editor: &mut Editor, kind: TrackKind, parent: Option<TrackId>) -> TrackId {
        let id = editor.new_id();
        let index = match (kind, parent) {
            (TrackKind::Return, _) => editor.document().returns().len(),
            (_, Some(group)) => editor.document().track(group).unwrap().children().len(),
            (_, None) => editor.document().top_level().len(),
        };
        editor
            .apply(Command::CreateTrack {
                id,
                kind,
                name: String::new(),
                parent,
                index,
            })
            .unwrap();
        id
    }

    fn input(editor: &mut Editor, track: TrackId, input: Input) -> Result<(), ModelError> {
        editor
            .apply(Command::SetInput { track, input })
            .map(|_| ())
            .map_err(|e| match e {
                crate::EditError::Command(e) => e,
                other => panic!("{other}"),
            })
    }

    fn output(editor: &mut Editor, track: TrackId, output: Output) -> Result<(), ModelError> {
        editor
            .apply(Command::SetOutput { track, output })
            .map(|_| ())
            .map_err(|e| match e {
                crate::EditError::Command(e) => e,
                other => panic!("{other}"),
            })
    }

    // Spec: routing R1
    #[test]
    fn a_new_tracks_routing_is_lives() {
        let mut editor = Editor::new();
        let audio = add(&mut editor, TrackKind::Audio, None);
        let midi = add(&mut editor, TrackKind::Midi, None);
        let doc = editor.document();
        let routing = *doc.track(audio).unwrap().routing();
        assert_eq!(routing.input, Input::External);
        assert_eq!(routing.monitor, Monitor::Auto);
        assert_eq!(routing.output, Output::Default);
        let routing = *doc.track(midi).unwrap().routing();
        assert_eq!(routing.input, Input::AllIns);
        assert_eq!(routing.monitor, Monitor::Auto);
        assert_eq!(routing.midi_output, None);
        assert_eq!(doc.output_of(audio), Some(doc.main_id()));
        assert_eq!(Tap::default(), Tap::PostMixer);
        assert_eq!(doc.track(doc.main_id()).unwrap().mixer().tap, SendTap::Post);
    }

    // Spec: routing R2, R3, R4
    #[test]
    fn each_kind_takes_the_inputs_and_outputs_its_choosers_list() {
        let mut editor = Editor::new();
        let a = add(&mut editor, TrackKind::Audio, None);
        let b = add(&mut editor, TrackKind::Audio, None);
        let m = add(&mut editor, TrackKind::Midi, None);
        let n = add(&mut editor, TrackKind::Midi, None);
        let g = add(&mut editor, TrackKind::Group, None);
        let inner = add(&mut editor, TrackKind::Audio, Some(g));
        let r = add(&mut editor, TrackKind::Return, None);
        let main = editor.document().main_id();
        let tap = Tap::PreFx;

        for from in [b, g, r, main] {
            input(&mut editor, a, Input::Track { track: from, tap }).unwrap();
        }
        for ok in [Input::Resampling, Input::External, Input::None] {
            input(&mut editor, a, ok).unwrap();
        }
        assert!(matches!(
            input(&mut editor, a, Input::Track { track: a, tap }),
            Err(ModelError::CannotRoute { .. })
        ));
        assert!(input(&mut editor, a, Input::Track { track: m, tap }).is_err());
        assert!(input(&mut editor, a, Input::AllIns).is_err());
        input(&mut editor, m, Input::Track { track: n, tap }).unwrap();
        input(&mut editor, m, Input::ComputerKeyboard).unwrap();
        assert!(input(&mut editor, m, Input::Track { track: a, tap }).is_err());
        assert!(input(&mut editor, m, Input::Resampling).is_err());
        assert!(input(&mut editor, g, Input::External).is_err());
        assert!(input(&mut editor, r, Input::External).is_err());

        // Audio To: Main, an audio track, the track's group, Sends Only, Ext. Out.
        for to in [
            Output::Main,
            Output::Track { track: b },
            Output::SendsOnly,
            Output::External,
        ] {
            output(&mut editor, a, to).unwrap();
            output(&mut editor, r, to).unwrap();
            output(&mut editor, g, to).unwrap();
        }
        output(&mut editor, g, Output::Track { track: inner }).unwrap();
        output(&mut editor, inner, Output::Main).unwrap();
        output(&mut editor, inner, Output::Track { track: g }).unwrap();
        assert_eq!(
            editor.document().track(inner).unwrap().routing().output,
            Output::Default,
            "a track's own group is its default"
        );
        assert!(output(&mut editor, a, Output::Track { track: g }).is_err());
        assert!(output(&mut editor, a, Output::Track { track: r }).is_err());
        assert!(output(&mut editor, a, Output::Track { track: a }).is_err());
        assert!(output(&mut editor, m, Output::SendsOnly).is_err());
        assert!(output(&mut editor, main, Output::SendsOnly).is_err());
        output(&mut editor, a, Output::Main).unwrap();
        assert_eq!(
            editor.document().track(a).unwrap().routing().output,
            Output::Default,
            "Main from the top level is the default"
        );

        editor
            .apply(Command::SetMidiOutput {
                track: m,
                to: Some(n),
            })
            .unwrap();
        assert!(
            editor
                .apply(Command::SetMidiOutput {
                    track: m,
                    to: Some(a)
                })
                .is_err()
        );
        assert!(
            editor
                .apply(Command::SetMidiOutput {
                    track: a,
                    to: Some(m)
                })
                .is_err()
        );
        assert!(
            editor
                .apply(Command::SetMonitor {
                    track: g,
                    monitor: Monitor::In
                })
                .is_err()
        );
        editor.document().check_invariants().unwrap();
    }

    // Spec: routing R5
    #[test]
    fn a_loop_is_kept_in_the_set() {
        let mut editor = Editor::new();
        let a = add(&mut editor, TrackKind::Audio, None);
        let b = add(&mut editor, TrackKind::Audio, None);
        input(
            &mut editor,
            a,
            Input::Track {
                track: b,
                tap: Tap::PostMixer,
            },
        )
        .unwrap();
        output(&mut editor, a, Output::Track { track: b }).unwrap();
        let saved = serde_json::to_value(editor.document()).unwrap();
        let loaded: Document = serde_json::from_value(saved).unwrap();
        assert_eq!(&loaded, editor.document());
    }

    // Spec: routing R6, R7
    #[test]
    fn grouping_and_deleting_carry_routings_as_live_does() {
        let mut editor = Editor::new();
        let a = add(&mut editor, TrackKind::Audio, None);
        let b = add(&mut editor, TrackKind::Audio, None);
        let c = add(&mut editor, TrackKind::Audio, None);
        let g = add(&mut editor, TrackKind::Group, None);
        let r = add(&mut editor, TrackKind::Return, None);
        output(&mut editor, b, Output::Track { track: c }).unwrap();
        output(&mut editor, r, Output::Track { track: c }).unwrap();
        input(
            &mut editor,
            a,
            Input::Track {
                track: c,
                tap: Tap::PostFx,
            },
        )
        .unwrap();
        // Grouping a track routed to Main sends it to the group; one routed
        // elsewhere stays.
        for track in [a, b] {
            editor
                .apply(Command::MoveTrack {
                    track,
                    parent: Some(g),
                    index: 0,
                })
                .unwrap();
        }
        let doc = editor.document();
        assert_eq!(doc.output_of(a), Some(g));
        assert_eq!(doc.output_of(b), Some(c));
        // Out of the group again, it goes to Main.
        editor
            .apply(Command::MoveTrack {
                track: a,
                parent: None,
                index: 0,
            })
            .unwrap();
        assert_eq!(
            editor.document().output_of(a),
            Some(editor.document().main_id())
        );

        let before = editor.document().clone();
        editor.apply(Command::DeleteTrack { track: c }).unwrap();
        let doc = editor.document();
        assert_eq!(doc.track(b).unwrap().routing().output, Output::SendsOnly);
        assert_eq!(doc.track(r).unwrap().routing().output, Output::SendsOnly);
        assert_eq!(doc.track(a).unwrap().routing().input, Input::None);
        doc.check_invariants().unwrap();
        editor.undo().unwrap();
        assert_eq!(editor.document(), &before, "undo restores the routings");
    }

    // Spec: routing R8
    #[test]
    fn routing_is_saved_only_where_it_differs_from_a_new_tracks() {
        let mut editor = Editor::new();
        let a = add(&mut editor, TrackKind::Audio, None);
        let m = add(&mut editor, TrackKind::Midi, None);
        let saved = serde_json::to_value(editor.document()).unwrap();
        assert!(saved["tracks"][0].get("routing").is_none());
        assert!(saved["tracks"][1].get("routing").is_none());
        editor
            .apply(Command::SetMonitor {
                track: a,
                monitor: Monitor::In,
            })
            .unwrap();
        input(&mut editor, a, Input::Resampling).unwrap();
        let saved = serde_json::to_value(editor.document()).unwrap();
        assert_eq!(
            saved["tracks"][0]["routing"],
            serde_json::json!({ "input": { "type": "resampling" }, "monitor": "in" })
        );
        let loaded: Document = serde_json::from_value(saved.clone()).unwrap();
        assert_eq!(&loaded, editor.document());
        assert_eq!(
            *loaded.track(m).unwrap().routing(),
            Routing::new(TrackKind::Midi)
        );
        // A routing that breaks a rule doesn't load.
        let mut broken = saved;
        broken["tracks"][0]["routing"]["input"] = serde_json::json!({ "type": "all_ins" });
        assert!(serde_json::from_value::<Document>(broken).is_err());
    }

    // Spec: routing R9 (IMP-01)
    #[test]
    fn fifty_return_tracks_save_and_load() {
        let mut editor = Editor::new();
        let a = add(&mut editor, TrackKind::Audio, None);
        let returns: Vec<TrackId> = (0..50)
            .map(|_| add(&mut editor, TrackKind::Return, None))
            .collect();
        for (i, &to) in returns.iter().enumerate() {
            editor
                .apply(Command::SetSend {
                    track: a,
                    to,
                    level: (i + 1) as f64 / 50.0,
                })
                .unwrap();
        }
        let doc = editor.document();
        assert_eq!(doc.returns().len(), 50);
        let loaded: Document = serde_json::from_str(&serde_json::to_string(doc).unwrap()).unwrap();
        assert_eq!(&loaded, doc);
        assert_eq!(loaded.track(a).unwrap().send(returns[49]), 1.0);
    }
}
