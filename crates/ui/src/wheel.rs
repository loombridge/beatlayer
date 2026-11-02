//! The mouse wheel on controls (WP-6.49; the view options spec, R10 to R13).
//!
//! With Adjust Values with Mouse Wheel on, the wheel over a knob, slider,
//! fader or number box moves it instead of scrolling what is under it. Every
//! such control calls [`input`] each frame: it tells this module the pointer
//! is over it, and returns the steps the wheel turned it by. Before the next
//! frame [`take`] takes the wheel out of the input for the control the
//! pointer was over, so no area scrolls with it, and keeps a run of turns on
//! one control in one undo step, which it hands the host as [`Gesture`]s to
//! run before the frame's keys and events.

use egui::{Context, Event, Id, MouseWheelUnit, RawInput, Rect, Response};

use bl_shell::Shell;

/// Points of a trackpad's scroll for one step: Beatlayer's own rate (R11).
const POINTS_PER_STEP: f32 = 20.0;

/// Seconds of the app's clock without a turn that end a run (R13).
const RUN_SECONDS: f64 = 0.5;

/// The control the pointer is over.
#[derive(Clone)]
struct Target {
    id: Id,
    rect: Rect,
    /// What an undo step of a run on it is called.
    name: String,
}

/// What the wheel did to a control this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Turn {
    /// Steps up (positive) or down: a notch is one, and so is every
    /// `POINTS_PER_STEP` points of a trackpad's scroll.
    pub steps: i32,
    /// Whether Shift was held, for finer steps.
    pub fine: bool,
}

/// A run's start or end, for the host to run on the shell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Gesture {
    /// The first turn of a run: what its undo step is called.
    Begin(String),
    /// The run is over.
    End,
}

impl Gesture {
    /// Opens or closes the run's undo step on `shell`.
    pub fn run(self, shell: &mut Shell) {
        match self {
            Self::Begin(name) => shell.begin_gesture(&name),
            Self::End => shell.end_gesture(),
        }
    }
}

#[derive(Clone, Default)]
struct State {
    /// The control the pointer was over as the last frame drew.
    seen: Option<Target>,
    /// The turn this frame's controls read, and whose it is.
    claim: Option<(Id, Turn)>,
    /// The run in progress: its control, where it is and when it last turned.
    run: Option<(Id, Rect, f64)>,
    /// A trackpad's scroll not yet worth a step.
    points: f32,
}

fn key() -> Id {
    Id::new("wheel-on-controls")
}

/// Says the pointer is over the control `response` belongs to, which takes
/// the wheel as `name` (the name of its undo step), and returns what the
/// wheel did to it this frame: the control then moves by `steps`, as the
/// arrow keys would, one of Shift's fine steps each with `fine`.
pub fn input(response: &Response, name: impl FnOnce() -> String) -> Option<Turn> {
    response.ctx.data_mut(|d| {
        let state = d.get_temp_mut_or_default::<State>(key());
        if response.hovered() {
            state.seen = Some(Target {
                id: response.id,
                rect: response.rect,
                name: name(),
            });
        }
        state
            .claim
            .filter(|(id, _)| *id == response.id)
            .map(|(_, turn)| turn)
    })
}

/// Takes the wheel out of `input` for the control the pointer is over, if
/// `enabled`, and starts or ends the run of turns on a control, which the
/// returned gestures say. Called before the keys are looked up, so a bound key
/// ends a run too.
pub fn take(input: &mut RawInput, ctx: &Context, enabled: bool) -> Vec<Gesture> {
    let mut gestures = Vec::new();
    let now = input.time.unwrap_or_default();
    let pointer = input
        .events
        .iter()
        .rev()
        .find_map(|event| match event {
            Event::PointerMoved(at) => Some(Some(*at)),
            Event::PointerGone => Some(None),
            _ => None,
        })
        .unwrap_or_else(|| ctx.input(|i| i.pointer.latest_pos()));
    // A key or a press of a button ends a run: the person has moved on.
    let moved_on = input.events.iter().any(|event| {
        matches!(
            event,
            Event::Key { pressed: true, .. } | Event::PointerButton { pressed: true, .. }
        )
    });
    let mut state = ctx.data_mut(|d| std::mem::take(d.get_temp_mut_or_default::<State>(key())));
    state.claim = None;
    let target = state
        .seen
        .take()
        .filter(|t| enabled && pointer.is_some_and(|p| t.rect.contains(p)));

    let mut turned = false;
    let mut turn = Turn {
        steps: 0,
        fine: false,
    };
    if target.is_some() {
        input.events.retain(|event| {
            let Event::MouseWheel {
                unit,
                delta,
                modifiers,
                ..
            } = event
            else {
                return true;
            };
            // Command with the wheel zooms the Arrangement.
            if modifiers.command {
                return true;
            }
            turned = true;
            // Shift turns a wheel's scroll sideways on some systems.
            let d = if delta.y != 0.0 {
                delta.y
            } else if modifiers.shift {
                delta.x
            } else {
                0.0
            };
            turn.fine |= modifiers.shift;
            match unit {
                MouseWheelUnit::Point => {
                    state.points += d;
                    let whole = (state.points / POINTS_PER_STEP).trunc();
                    state.points -= whole * POINTS_PER_STEP;
                    turn.steps += whole as i32;
                }
                MouseWheelUnit::Line | MouseWheelUnit::Page => {
                    if d != 0.0 {
                        turn.steps += (d.signum() * d.abs().round().max(1.0)) as i32;
                    }
                }
            }
            false
        });
    } else {
        state.points = 0.0;
    }

    if let Some((id, rect, last)) = state.run {
        let left = !pointer.is_some_and(|p| rect.contains(p));
        let other = turned && target.as_ref().is_some_and(|t| t.id != id);
        if !enabled || left || moved_on || other || now - last >= RUN_SECONDS {
            gestures.push(Gesture::End);
            state.run = None;
        }
    }
    if let Some(target) = target
        && turned
    {
        match &mut state.run {
            Some((_, _, last)) => *last = now,
            None => {
                gestures.push(Gesture::Begin(target.name.clone()));
                state.run = Some((target.id, target.rect, now));
            }
        }
        if turn.steps != 0 {
            state.claim = Some((target.id, turn));
        }
    }
    if state.run.is_some() {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(RUN_SECONDS));
    }
    ctx.data_mut(|d| d.insert_temp(key(), state));
    gestures
}
