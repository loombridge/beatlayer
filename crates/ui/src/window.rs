//! Which of the two main windows a context draws (WP-6.29): the main window
//! or the second one, each a [`MainWindowUi`](crate::main_window::MainWindowUi)
//! on a context of its own. The panes it draws ask here which window's focus
//! and main view to read.

use bl_shell::{Area, Shell, WindowKind};
use egui::{Context, Id};

/// Records that `ctx` draws `window`.
pub fn set(ctx: &Context, window: WindowKind) {
    ctx.data_mut(|data| data.insert_temp(Id::NULL, window));
}

/// The window `ctx` draws: the main window unless [`set`] said otherwise.
#[must_use]
pub fn of(ctx: &Context) -> WindowKind {
    ctx.data(|data| data.get_temp(Id::NULL))
        .unwrap_or(WindowKind::Main)
}

/// The area of the window `ctx` draws that has the keyboard focus.
#[must_use]
pub fn focused_area(ctx: &Context, shell: &Shell) -> Area {
    shell.focused_area_of(of(ctx))
}

/// Whether `area` of the window `ctx` draws has the keyboard focus, and the
/// window is the one in front.
#[must_use]
pub fn has_focus(ctx: &Context, shell: &Shell, area: Area) -> bool {
    let window = of(ctx);
    shell.focused_window() == window && shell.focused_area_of(window) == area
}

/// The main view of the window `ctx` draws.
#[must_use]
pub fn main_view(ctx: &Context, shell: &Shell) -> bl_shell::MainView {
    shell.main_view_of(of(ctx))
}
