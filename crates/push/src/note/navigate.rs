//! Navigating in Note Mode: the arrows select the neighbouring tracks and
//! scenes (`docs/specs/push-2-notes.md`, R45 and R46).

use bl_control::ReadModel;
use bl_model::SceneId;
use serde_json::json;

use super::{Note, clip};
use crate::content::{Ctx, DisplayMode, MixPage};
use crate::controls::cc;
use crate::curve::Workflow;

/// How many tracks the surface's ring is wide.
const RING: usize = 8;

impl Note {
    /// An arrow pressed. Browse Mode has the arrows for its lists, and Global
    /// Mix the left and right ones for its tracks.
    pub(super) fn arrow(&mut self, button: u8, ctx: &mut Ctx<'_>) {
        let sideways = matches!(button, cc::LEFT | cc::RIGHT);
        if ctx.display == DisplayMode::Browse
            || (sideways && ctx.display == DisplayMode::Mix && ctx.mix_page == MixPage::Global)
        {
            return;
        }
        if sideways {
            Self::select_neighbour(button == cc::RIGHT, ctx);
        } else {
            self.launch_neighbour(button == cc::DOWN, ctx);
        }
    }

    /// Left and Right select the track before or after the selected one, and
    /// the ring moves a track to keep it among its eight (R45).
    fn select_neighbour(right: bool, ctx: &mut Ctx<'_>) {
        let Some(model) = ctx.model else {
            return;
        };
        let Some(selected) = clip::selected_track(model) else {
            return;
        };
        let Some(at) = model.columns.iter().position(|t| *t == selected) else {
            return;
        };
        let to = if right {
            at + 1
        } else {
            let Some(to) = at.checked_sub(1) else {
                return;
            };
            to
        };
        let Some(&track) = model.columns.get(to) else {
            return;
        };
        ctx.select_track(track);
        let first = model
            .row(ctx.cx.row())
            .and_then(|row| row.ring.as_ref())
            .and_then(|ring| ring.tracks.first())
            .and_then(|first| model.columns.iter().position(|t| t == first));
        let step = match first {
            Some(first) if to < first => "left",
            Some(first) if to >= first + RING => "right",
            _ => return,
        };
        ctx.cx.run(
            "control_surface.move_ring",
            json!({ "row": ctx.cx.row() + 1, "step": step }),
        );
    }

    /// The scene above or below the selected one, if there is one (R46).
    fn neighbour_scene(model: &ReadModel, scene: Option<SceneId>, down: bool) -> Option<SceneId> {
        let scenes: Vec<SceneId> = model.document.scenes().map(|s| s.id).collect();
        let at = scenes.iter().position(|s| Some(*s) == scene)?;
        let to = if down { at + 1 } else { at.checked_sub(1)? };
        scenes.get(to).copied()
    }

    /// Whether Up or Down can move: the first scene has none above it and the
    /// last none below (R46).
    pub(super) fn can_step_scene(model: &ReadModel, scene: Option<SceneId>, down: bool) -> bool {
        Self::neighbour_scene(model, scene, down).is_some()
    }

    /// Up and Down select the scene above or below and launch it at once, as
    /// Legato: with Workflow on Scene every clip of the scene, on Clip only
    /// the selected track's (R46).
    fn launch_neighbour(&mut self, down: bool, ctx: &mut Ctx<'_>) {
        let Some(model) = ctx.model else {
            return;
        };
        let Some(frame) = self.frame(model) else {
            return;
        };
        let Some(scene) = Self::neighbour_scene(model, frame.scene, down) else {
            return;
        };
        let doc = &model.document;
        let tracks: Vec<_> = match ctx.setup.workflow {
            Workflow::Scene => doc
                .track_order()
                .into_iter()
                .filter(|t| clip::slot_holds(model, *t, scene))
                .collect(),
            Workflow::Clip => vec![frame.track],
        };
        match ctx.setup.workflow {
            Workflow::Scene => ctx.cx.run("session.select", json!({ "scene": scene })),
            Workflow::Clip => ctx.cx.run(
                "session.select",
                json!({ "track": frame.track, "scene": scene }),
            ),
        }
        for track in tracks
            .into_iter()
            .filter(|t| clip::slot_holds(model, *t, scene))
        {
            ctx.cx.run(
                "session.launch",
                json!({ "track": track, "scene": scene, "legato": true }),
            );
        }
    }
}
