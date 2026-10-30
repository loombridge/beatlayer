//! The relative Session mapping strip's commands and the New button (WP-7.3b;
//! the map modes spec, R14 to R16 and R20): highlighting a scene, launching
//! the highlighted scene or a track's slot at it, and readying a slot for a
//! new take.
//!
//! The highlighted scene is the Session View's selected scene, so every
//! highlight is a `session.select`; what these add is the step, the grid
//! scrolling to centre it, and the launches from it.

use bl_model::{SceneId, TrackId};
use bl_shell_spec::session_strip::{
    self as ids, HighlightArgs, Highlighted, LaunchHighlightedArgs, NewTake,
};
use bl_shell_spec::session_view as session_ids;
use serde_json::{Value, json};

use super::*;

impl Shell {
    /// The highlighted scene: the selected scene, or the scene of the first
    /// selected slot (map modes R15).
    #[must_use]
    pub fn highlighted_scene(&self) -> Option<SceneId> {
        self.session.highlighted()
    }

    /// Highlights `scene` and has the grid centre it (R15, R16).
    fn highlight(&mut self, scene: SceneId) -> Result<(), ShellError> {
        self.run(session_ids::SELECT, json!({ "scene": scene }))?;
        self.session.request_centre(scene);
        Ok(())
    }

    /// Runs the strip's commands and New.
    pub(super) fn run_session_strip(
        &mut self,
        id: &str,
        args: Value,
    ) -> Result<Value, ShellError> {
        let unavailable = |reason: &str| ShellError::Unavailable {
            command: id.to_owned(),
            reason: reason.to_owned(),
        };
        let scenes: Vec<SceneId> = self
            .editor
            .document()
            .scenes()
            .map(|scene| scene.id)
            .collect();
        let at = self
            .highlighted_scene()
            .and_then(|scene| scenes.iter().position(|s| *s == scene));
        match id {
            ids::HIGHLIGHT_SCENE => {
                let HighlightArgs { step, number } = Self::args(id, args)?;
                let last = scenes.len().checked_sub(1).ok_or_else(|| {
                    unavailable("the set has no scenes")
                })?;
                let to = match (step, number) {
                    // Spec: map-modes R15. With none highlighted a step
                    // highlights the first.
                    (Some(step), None) => at.map_or(0, |at| {
                        (at as i64 + i64::from(step)).clamp(0, last as i64) as usize
                    }),
                    (None, Some(number)) => (number as usize)
                        .checked_sub(1)
                        .filter(|at| *at <= last)
                        .ok_or_else(|| ShellError::InvalidArgs {
                            command: id.to_owned(),
                            message: format!("`number` is from 1 to {}", last + 1),
                        })?,
                    _ => {
                        return Err(ShellError::InvalidArgs {
                            command: id.to_owned(),
                            message: "give `step` or `number`".to_owned(),
                        });
                    }
                };
                self.highlight(scenes[to])?;
                Ok(to_value(&Highlighted {
                    scene: scenes[to],
                    number: to as u32 + 1,
                }))
            }
            ids::LAUNCH_HIGHLIGHTED_SCENE => {
                Self::args::<bl_commands::NoArgs>(id, args)?;
                let scene = self
                    .highlighted_scene()
                    .ok_or_else(|| unavailable("highlight a scene first"))?;
                self.run("session.launch_scene", json!({ "scene": scene }))?;
                // Select Next Scene on Launch moved the highlight on.
                if let Some(now) = self.highlighted_scene().filter(|now| *now != scene) {
                    self.session.request_centre(now);
                }
                Ok(Value::Null)
            }
            ids::LAUNCH_HIGHLIGHTED => {
                let LaunchHighlightedArgs {
                    track,
                    velocity,
                    transpose,
                } = Self::args(id, args)?;
                let scene = self
                    .highlighted_scene()
                    .ok_or_else(|| unavailable("highlight a scene first"))?;
                self.run(
                    "session.launch",
                    json!({
                        "track": track,
                        "scene": scene,
                        "velocity": velocity,
                        "transpose": transpose,
                    }),
                )?;
                Ok(Value::Null)
            }
            _ => {
                Self::args::<bl_commands::NoArgs>(id, args)?;
                self.new_take(&scenes, at)
            }
        }
    }

    /// `session.new_take` (R20): stops the armed tracks' clips, then
    /// highlights the first scene after the highlighted one whose slots on
    /// those tracks are empty, inserting one after the last when there is
    /// none.
    fn new_take(&mut self, scenes: &[SceneId], at: Option<usize>) -> Result<Value, ShellError> {
        let doc = self.editor.document();
        let armed: Vec<TrackId> = doc
            .tracks()
            .filter(|track| track.kind().has_arm() && track.mixer().arm)
            .map(bl_model::Track::id)
            .collect();
        let free = |scene: SceneId| armed.iter().all(|track| doc.slot(*track, scene).is_none());
        let found = scenes
            .iter()
            .enumerate()
            .skip(at.map_or(0, |at| at + 1))
            .find_map(|(_, scene)| free(*scene).then_some(*scene));
        for track in &armed {
            // With no engine nothing plays, so there is nothing to stop.
            let _ = self.run("session.stop_track", json!({ "track": track }));
        }
        let (scene, inserted) = match found {
            Some(scene) => (scene, false),
            None => {
                self.run(session_ids::INSERT_SCENE, json!({ "index": scenes.len() }))?;
                let made = self
                    .editor
                    .document()
                    .scenes()
                    .last()
                    .map(|scene| scene.id)
                    .ok_or_else(|| ShellError::Failed {
                        command: ids::NEW_TAKE.to_owned(),
                        message: "no scene was inserted".to_owned(),
                    })?;
                (made, true)
            }
        };
        self.highlight(scene)?;
        Ok(to_value(&NewTake {
            scene,
            inserted,
            stopped: armed,
        }))
    }
}
