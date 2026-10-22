//! Panning with the middle mouse button: Live 12 pans any area that
//! scrolls by holding the mouse wheel down and dragging (the view options
//! spec, R1; I-DRAG-058).
//!
//! The button's presses are taken out of the input before egui sees them,
//! so a middle-button drag clicks nothing, selects nothing and moves
//! nothing: the area under the pointer pans with the pointer, and every
//! other thing in the window keeps its own behaviour.

use egui::{Context, Event, Id, PointerButton, Pos2, RawInput, Rect, Ui, Vec2};

/// The middle button's state, kept in the egui context so every area can
/// read it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Middle {
    /// Whether the button is down.
    down: bool,
    /// Where it went down.
    origin: Option<Pos2>,
    /// Whether it went down in this frame: that frame pans nothing, so
    /// the movement that reached the button isn't panned with.
    pressed: bool,
}

/// The ID the state is kept under.
fn id() -> Id {
    Id::new("middle-button")
}

/// Takes the middle button's presses out of `input` and keeps its state in
/// `ctx`, so egui never sees the button: a middle-button drag pans and
/// clicks nothing.
pub fn take(input: &mut RawInput, ctx: &Context) {
    let mut middle = ctx.data(|d| d.get_temp::<Middle>(id())).unwrap_or_default();
    middle.pressed = false;
    input.events.retain(|event| {
        let Event::PointerButton {
            pos,
            button: PointerButton::Middle,
            pressed,
            ..
        } = event
        else {
            return true;
        };
        if *pressed {
            middle.down = true;
            middle.origin = Some(*pos);
            middle.pressed = true;
        } else {
            middle.down = false;
            middle.origin = None;
        }
        false
    });
    ctx.data_mut(|d| d.insert_temp(id(), middle));
}

/// The pan of the area under the pointer: how far the pointer moved since
/// the last frame, when the middle button went down over `area`.
///
/// The area that took the button pans, and keeps panning until the button
/// comes up, wherever the pointer goes (the view options spec, R1).
#[must_use]
pub fn pan(ui: &Ui, area: Rect) -> Option<Vec2> {
    let middle = ui
        .ctx()
        .data(|d| d.get_temp::<Middle>(id()))
        .unwrap_or_default();
    if !middle.down {
        return None;
    }
    let origin = middle.origin?;
    if !area.contains(origin) {
        return None;
    }
    let (delta, latest) = ui.input(|i| (i.pointer.delta(), i.pointer.latest_pos()));
    if middle.pressed {
        // The frame the button went down: the content moves with the
        // pointer from where it went down, and not with the movement that
        // reached the button.
        return Some(latest? - origin);
    }
    Some(delta)
}
