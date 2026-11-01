//! New and Duplicate on Push 2, and Fixed Length switched on during a
//! recording (`docs/specs/push-2-notes.md`, R50 to R53).

use std::collections::BTreeSet;

use bl_model::{SceneId, TrackId, TrackKind};
use bl_push::curve::Workflow;
use bl_shell_spec::push as ids;

use super::*;

impl Shell {
    /// The Workflow `args` ask for, else the Setup menu's.
    fn push_workflow(&self, args: &ids::PushSessionArgs) -> Workflow {
        match args.workflow {
            Some(ids::PushWorkflow::Scene) => Workflow::Scene,
            Some(ids::PushWorkflow::Clip) => Workflow::Clip,
            None => self.midi.push().setup().workflow,
        }
    }

    fn scene_ids(&self) -> Vec<SceneId> {
        self.editor.document().scenes().map(|s| s.id).collect()
    }

    /// Runs `steps` as one undo step, named `label`.
    fn one_step(
        &mut self,
        label: &'static str,
        steps: impl FnOnce(&mut Self) -> Result<(), ShellError>,
    ) -> Result<Value, ShellError> {
        self.editor.begin(label);
        let result = steps(self);
        match result {
            Ok(()) => {
                let _ = self.editor.commit();
                Ok(Value::Null)
            }
            Err(error) => {
                let _ = self.editor.cancel();
                self.document_changed();
                Err(error)
            }
        }
    }

    /// `push.new` (R52).
    pub(super) fn push_new(&mut self, id: &str, args: ids::PushSessionArgs) -> Result<Value, ShellError> {
        let unavailable = |reason: &str| ShellError::Unavailable {
            command: id.to_owned(),
            reason: reason.to_owned(),
        };
        let workflow = self.push_workflow(&args);
        let track: TrackId = self
            .arrangement
            .selected_tracks()
            .first()
            .copied()
            .filter(|&t| {
                self.editor
                    .document()
                    .track(t)
                    .is_some_and(|t| matches!(t.kind(), TrackKind::Midi | TrackKind::Audio))
            })
            .ok_or_else(|| unavailable("select a MIDI or audio track first"))?;
        let from = self.session_scene();
        self.one_step("New", |shell| {
            // With no engine nothing plays, so there is nothing to stop.
            let _ = shell.run("session.stop_track", json!({ "track": track }));
            let scene = match workflow {
                Workflow::Scene => {
                    let before: BTreeSet<SceneId> = shell.scene_ids().into_iter().collect();
                    shell.run(
                        "session.capture_and_insert_scene",
                        json!({ "except": [track] }),
                    )?;
                    shell.scene_ids().into_iter().find(|s| !before.contains(s))
                }
                Workflow::Clip => {
                    let scenes = shell.scene_ids();
                    let at = from.and_then(|from| scenes.iter().position(|s| *s == from));
                    let doc = shell.editor.document();
                    let empty = scenes
                        .iter()
                        .skip(at.map_or(0, |at| at + 1))
                        .copied()
                        .find(|&s| doc.slot(track, s).is_none());
                    match empty {
                        Some(scene) => Some(scene),
                        None => {
                            shell.run("session.insert_scene", json!({ "index": scenes.len() }))?;
                            shell.scene_ids().last().copied()
                        }
                    }
                }
            };
            let scene = scene.ok_or_else(|| ShellError::Failed {
                command: id.to_owned(),
                message: "no scene was made".to_owned(),
            })?;
            shell.run("session.select", json!({ "track": track, "scene": scene }))?;
            Ok(())
        })
    }

    /// `push.duplicate` (R53).
    pub(super) fn push_duplicate(
        &mut self,
        id: &str,
        args: ids::PushSessionArgs,
    ) -> Result<Value, ShellError> {
        if args.scene || self.push_workflow(&args) == Workflow::Scene {
            return self.run("session.capture_and_insert_scene", json!({}));
        }
        let unavailable = |reason: &str| ShellError::Unavailable {
            command: id.to_owned(),
            reason: reason.to_owned(),
        };
        let (track, scene) = self
            .session
            .state()
            .slots
            .first()
            .map(|slot| (slot.track, slot.scene))
            .ok_or_else(|| unavailable("select a clip first"))?;
        let clip = self
            .editor
            .document()
            .slot(track, scene)
            .map(|clip| clip.id)
            .ok_or_else(|| unavailable("the selected slot holds no clip"))?;
        self.one_step("Duplicate", |shell| {
            let scenes = shell.scene_ids();
            let at = scenes.iter().position(|s| *s == scene).unwrap_or(0);
            let below = scenes.get(at + 1).copied();
            let target = match below {
                Some(below) if shell.editor.document().slot(track, below).is_none() => below,
                _ => {
                    shell.run("session.insert_scene", json!({ "index": at + 1 }))?;
                    shell.scene_ids()[at + 1]
                }
            };
            shell.run(
                "session.copy_clip_to_slot",
                json!({ "clip": clip, "track": track, "scene": target }),
            )?;
            // A Legato launch, leaving the other tracks playing.
            let _ = shell.run(
                "session.launch",
                json!({ "track": track, "scene": target, "legato": true }),
            );
            Ok(())
        })
    }
}
