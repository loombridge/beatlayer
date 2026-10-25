//! The follow-action controls (WP-3.10), shared by the Clip View's Launch
//! Properties panel (L-CLIP-005) and the Scene View (L-SES-011): the Follow
//! Action switch, actions A and B with their chances and, while an action
//! is Jump, its target, and the follow time with Linked (Longest on a
//! scene).
//!
//! Each control edits through `clip.set_follow_actions` or
//! `scene.set_follow_actions` and shows the saved settings back. The follow
//! times are counted in 4/4 bars here, as the command's `time` is in ticks.

use bl_core::time::Ticks;
use bl_model::{
    ClipId, FOLLOW_MULTIPLIER_MAX, FOLLOW_TIME_MAX, FOLLOW_TIME_MIN, FollowAction, FollowActions,
    SceneId,
};
use bl_shell::{Area, Invocation};
use egui::{Align2, Id, Pos2, Rect, Ui, Vec2, pos2, vec2};
use serde_json::{Value, json};

use crate::main_window::UiEvent;
use crate::theme::Theme;
use crate::widgets::{self, Choice, NO_LEDS};

const ROW: f32 = 19.0;

/// The sizes of the controls: a chooser's, a chance's or a switch's, a jump
/// target's, the follow time's, and where the controls start from the
/// labels' left edge.
struct Metrics {
    chooser: Vec2,
    small: Vec2,
    jump: Vec2,
    time: Vec2,
    controls_at: f32,
    /// Whether a Jump action's target gets a row of its own, for a panel too
    /// narrow to hold it beside the chooser and the chance.
    jump_row: bool,
}

/// The Scene View's.
const WIDE: Metrics = Metrics {
    chooser: vec2(120.0, 17.0),
    small: vec2(54.0, 17.0),
    jump: vec2(34.0, 17.0),
    time: vec2(70.0, 17.0),
    controls_at: 92.0,
    jump_row: false,
};

/// The Clip View's left group, which is 166 points wide (L-CLIP-004).
const NARROW: Metrics = Metrics {
    chooser: vec2(62.0, 17.0),
    small: vec2(38.0, 17.0),
    jump: vec2(38.0, 17.0),
    time: vec2(56.0, 17.0),
    controls_at: 50.0,
    jump_row: true,
};

/// Whose follow actions the controls edit.
#[derive(Clone, Copy)]
pub enum Owner {
    /// A Session clip. `loop_off` is true when its Loop switch is off, when
    /// Linked reads "Clip End".
    Clip(ClipId, bool),
    /// A scene, whose Linked is Longest.
    Scene(SceneId),
}

/// `bars.beats.sixteenths` for a length in ticks: 1.0.0 is one 4/4 bar.
#[must_use]
pub fn bars_beats_sixteenths(time: Ticks) -> String {
    let t = time.get();
    let (bar, beat, sixteenth) = (
        Ticks::WHOLE.get(),
        Ticks::QUARTER.get(),
        Ticks::SIXTEENTH.get(),
    );
    format!("{}.{}.{}", t / bar, t % bar / beat, t % beat / sixteenth)
}

/// Draws the controls from `at` down, `prefix` ahead of every control's
/// accessible name, and returns the y below them. `scenes` is how many
/// scenes a Jump can name.
#[expect(clippy::too_many_arguments, reason = "the frame's inputs")]
pub fn show(
    ui: &mut Ui,
    theme: &Theme,
    at: Pos2,
    owner: Owner,
    follow: &FollowActions,
    prefix: &str,
    scenes: usize,
    events: &mut Vec<UiEvent>,
) -> f32 {
    let c = &theme.colors;
    let m = match owner {
        Owner::Clip(..) => NARROW,
        Owner::Scene(_) => WIDE,
    };
    let painter = ui.painter().clone();
    let mut y = at.y;
    let mut row = |label: &str| {
        painter.text(
            pos2(at.x, y + m.chooser.y / 2.0),
            Align2::LEFT_CENTER,
            label,
            theme.small_font(),
            c.text_dim.0,
        );
        let start = pos2(at.x + m.controls_at, y);
        y += ROW;
        start
    };
    let name = |what: &str| format!("{prefix}{what}");
    let change = |field: &str, value: Value| {
        let (command, key, id) = match owner {
            Owner::Clip(clip, _) => ("clip.set_follow_actions", "clip", json!(clip)),
            Owner::Scene(scene) => ("scene.set_follow_actions", "scene", json!(scene)),
        };
        UiEvent::Invoke(Invocation::new(command, json!({ key: id, field: value })))
    };
    let focus = |ui: &Ui, kind: &str, salt: &str| {
        crate::focus::register(
            ui.ctx(),
            Id::new((kind, salt.to_owned())),
            Area::Detail,
            None,
        );
    };

    // The Follow Action switch.
    let start = row("");
    let salt = name("follow-enabled");
    focus(ui, "switch", &salt);
    if let Some(on) = widgets::switch(
        ui,
        theme,
        &salt,
        &name("Follow Action"),
        Rect::from_min_size(start, m.small),
        follow.enabled,
    ) {
        events.push(change("enabled", json!(on)));
    }

    // Actions A and B, each with its Jump target and its chance.
    let actions: Vec<_> = FollowAction::ALL
        .into_iter()
        .map(|action| Choice {
            label: action.label().to_owned(),
            value: action,
            leds: NO_LEDS,
        })
        .collect();
    for (side, action, jump, chance, field, jump_field) in [
        (
            "A",
            follow.action_a,
            follow.jump_a,
            follow.chance_a,
            "action_a",
            "jump_a",
        ),
        (
            "B",
            follow.action_b,
            follow.jump_b,
            follow.chance_b(),
            "action_b",
            "jump_b",
        ),
    ] {
        let start = row(&format!("Action {side}"));
        let salt = name(&format!("follow-action-{side}"));
        focus(ui, "chooser", &salt);
        if let Some(chosen) = widgets::chooser(
            ui,
            theme,
            &salt,
            &name(&format!("Follow Action {side}")),
            Rect::from_min_size(start, m.chooser),
            &actions,
            Some(&action),
            true,
        ) {
            events.push(change(field, json!(chosen)));
        }
        let mut next = start.x + m.chooser.x + if m.jump_row { 3.0 } else { 6.0 };
        if action == FollowAction::Jump {
            let salt = name(&format!("follow-jump-{side}"));
            focus(ui, "number-box", &salt);
            // Beside the chooser, or on a row of its own under it.
            let at = if m.jump_row {
                row(&format!("Jump {side}"))
            } else {
                pos2(next, start.y)
            };
            if let Some(target) = widgets::number_box(
                ui,
                theme,
                &salt,
                &name(&format!("Jump {side}")),
                Rect::from_min_size(at, m.jump),
                f64::from(jump),
                (0.0, scenes as f64),
                &|value, steps| value + f64::from(steps),
                &if jump == 0 {
                    "-".to_owned()
                } else {
                    jump.to_string()
                },
            ) {
                events.push(change(jump_field, json!(target.round() as u32)));
            }
            if !m.jump_row {
                next += m.jump.x + 6.0;
            }
        }
        let salt = name(&format!("follow-chance-{side}"));
        focus(ui, "number-box", &salt);
        if let Some(percent) = widgets::number_box(
            ui,
            theme,
            &salt,
            &name(&format!("Chance {side}")),
            Rect::from_min_size(pos2(next, start.y), m.small),
            f64::from(chance),
            (0.0, 100.0),
            &|value, steps| value + f64::from(steps),
            &format!("{chance} %"),
        ) {
            // The two always add up to 100: B moves A the other way.
            let a = if side == "A" {
                percent.round()
            } else {
                100.0 - percent.round()
            };
            events.push(change("chance_a", json!(a as u8)));
        }
    }

    // The follow time: Linked (Longest on a scene), then the multiplier, or
    // Clip End, or the Unlinked time.
    let (switch_name, switch_label) = match owner {
        Owner::Scene(_) => ("Follow Time Longest", "Longest"),
        Owner::Clip(..) => ("Follow Time Linked", "Linked"),
    };
    let start = row(switch_label);
    let salt = name("follow-linked");
    focus(ui, "switch", &salt);
    if let Some(linked) = widgets::switch(
        ui,
        theme,
        &salt,
        &name(switch_name),
        Rect::from_min_size(start, m.small),
        follow.linked,
    ) {
        events.push(change("linked", json!(linked)));
    }
    let beside = pos2(start.x + m.small.x + 3.0, start.y);
    if follow.linked {
        if matches!(owner, Owner::Clip(_, true)) {
            painter_text(ui, theme, beside + vec2(0.0, m.chooser.y / 2.0), "Clip End");
        } else {
            let salt = name("follow-multiplier");
            focus(ui, "number-box", &salt);
            if let Some(multiplier) = widgets::number_box(
                ui,
                theme,
                &salt,
                &name("Follow Time Multiplier"),
                Rect::from_min_size(beside, m.small),
                f64::from(follow.multiplier),
                (1.0, f64::from(FOLLOW_MULTIPLIER_MAX)),
                &|value, steps| value + f64::from(steps),
                &format!("{}x", follow.multiplier),
            ) {
                events.push(change("multiplier", json!(multiplier.round() as u32)));
            }
        }
    } else {
        let salt = name("follow-time");
        focus(ui, "number-box", &salt);
        let step = Ticks::SIXTEENTH.get() as f64;
        if let Some(time) = widgets::number_box(
            ui,
            theme,
            &salt,
            &name("Follow Time"),
            Rect::from_min_size(beside, m.time),
            follow.time.get() as f64,
            (FOLLOW_TIME_MIN.get() as f64, FOLLOW_TIME_MAX.get() as f64),
            &|value, steps| value + f64::from(steps) * step,
            &bars_beats_sixteenths(follow.time),
        ) {
            events.push(change("time", json!(time.round() as i64)));
        }
    }
    y
}

/// Plain text at `at`, vertically centred.
fn painter_text(ui: &Ui, theme: &Theme, at: Pos2, text: &str) {
    ui.painter().text(
        at,
        Align2::LEFT_CENTER,
        text,
        theme.small_font(),
        theme.colors.text.0,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_read_in_bars_beats_and_sixteenths() {
        // Spec: follow-actions R9. 1.0.0 is a bar, 0.0.1 the shortest.
        assert_eq!(bars_beats_sixteenths(Ticks::WHOLE), "1.0.0");
        assert_eq!(bars_beats_sixteenths(Ticks::SIXTEENTH), "0.0.1");
        assert_eq!(
            bars_beats_sixteenths(Ticks::WHOLE * 3 + Ticks::QUARTER * 2 + Ticks::SIXTEENTH * 3),
            "3.2.3"
        );
    }
}
