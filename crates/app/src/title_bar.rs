//! Live's compact title bar on macOS (D22, L-WIN-002): 23 points tall, the
//! window buttons at the left, the set's name centred.
//!
//! The window's content view covers the whole frame (winit's full-size
//! content view, with the system's title bar transparent and its title
//! hidden), and Beatlayer draws the title area itself (`bl_ui`). What stays
//! the system's is the title bar's behaviour: AppKit's title bar view still
//! sits over the top of the window, so dragging moves the window,
//! double-clicking zooms it (or minimises it, by the person's setting), and
//! the close, minimise, and full-screen buttons work as in any window. This
//! module shrinks that view from the system's height (32 points on macOS 26)
//! to Live's 23 and centres the buttons in it, so clicks below it reach the
//! Control Bar. AppKit lays the title bar out again when the window resizes
//! or leaves full screen, so the host calls [`fit`] after each resize.

use bl_ui::layout::TITLE_BAR;
use objc2_app_kit::{NSView, NSWindow, NSWindowButton};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

/// The window's AppKit window, from its content view.
fn ns_window(window: &Window) -> Option<objc2::rc::Retained<NSWindow>> {
    let handle = window.window_handle().ok()?;
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return None;
    };
    // SAFETY: winit's AppKit handle points at the window's content view,
    // which lives as long as the window, and this runs on the main thread
    // (winit only hands out window handles there on macOS).
    let view: &NSView = unsafe { handle.ns_view.cast().as_ref() };
    view.window()
}

/// Fits the system's title bar to Live's 23 points: the strip that drags
/// and zooms the window, with the window buttons centred in it. Does
/// nothing in full screen, where the system shows its own title bar on
/// demand and Live shows none (L-WIN-002).
pub fn fit(window: &Window) {
    if window.fullscreen().is_some() {
        return;
    }
    let Some(ns_window) = ns_window(window) else {
        return;
    };
    let height = f64::from(TITLE_BAR);
    let Some(close) = ns_window.standardWindowButton(NSWindowButton::CloseButton) else {
        return;
    };
    // The buttons sit in the title bar view, which sits in its container.
    // SAFETY: the views are AppKit's, alive while the window is; nothing
    // here keeps them past this call.
    let Some(title_view) = (unsafe { close.superview() }) else {
        return;
    };
    // SAFETY: as above.
    let Some(container) = (unsafe { title_view.superview() }) else {
        return;
    };
    let window_height = ns_window.frame().size.height;
    let mut frame = container.frame();
    frame.size.height = height;
    frame.origin.y = window_height - height;
    container.setFrame(frame);
    let mut inner = title_view.frame();
    inner.origin.y = 0.0;
    inner.size.height = height;
    title_view.setFrame(inner);
    for kind in [
        NSWindowButton::CloseButton,
        NSWindowButton::MiniaturizeButton,
        NSWindowButton::ZoomButton,
    ] {
        if let Some(button) = ns_window.standardWindowButton(kind) {
            let size = button.frame().size;
            let mut origin = button.frame().origin;
            origin.y = ((height - size.height) / 2.0).round();
            button.setFrameOrigin(origin);
        }
    }
}

/// What AppKit placed, for `BEATLAYER_TRACE=1`: the title bar's strip and
/// each window button, in points from the window's top-left corner, and the
/// items the Edit menu shows at its end (where macOS adds Emoji & Symbols).
pub fn describe(window: &Window) -> String {
    let Some(ns_window) = ns_window(window) else {
        return "no AppKit window".to_owned();
    };
    let window_height = ns_window.frame().size.height;
    let mut out = Vec::new();
    if let Some(close) = ns_window.standardWindowButton(NSWindowButton::CloseButton)
        // SAFETY: as in `fit`.
        && let Some(title_view) = unsafe { close.superview() }
        // SAFETY: as in `fit`.
        && let Some(container) = unsafe { title_view.superview() }
    {
        let frame = container.frame();
        out.push(format!(
            "title bar y {:.0} to {:.0}",
            window_height - frame.origin.y - frame.size.height,
            window_height - frame.origin.y
        ));
    }
    for (name, kind) in [
        ("close", NSWindowButton::CloseButton),
        ("minimise", NSWindowButton::MiniaturizeButton),
        ("full screen", NSWindowButton::ZoomButton),
    ] {
        if let Some(button) = ns_window.standardWindowButton(kind) {
            // In the title bar view's coordinates, which start at the
            // window's top edge once `fit` has run.
            let frame = button.frame();
            let top = f64::from(TITLE_BAR) - frame.origin.y - frame.size.height;
            out.push(format!(
                "{name} x {:.0} to {:.0}, y {:.0} to {:.0}",
                frame.origin.x,
                frame.origin.x + frame.size.width,
                top,
                top + frame.size.height
            ));
        }
    }
    if let Some(mtm) = objc2::MainThreadMarker::new() {
        let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
        if let Some(menu) = app.mainMenu() {
            for index in 0..menu.numberOfItems() {
                let Some(item) = menu.itemAtIndex(index) else {
                    continue;
                };
                if item.title().to_string() != "Edit" {
                    continue;
                }
                if let Some(edit) = item.submenu() {
                    // The items a person sees: AppKit also adds hidden and
                    // alternate copies of its own.
                    let shown: Vec<String> = (0..edit.numberOfItems())
                        .filter_map(|i| edit.itemAtIndex(i))
                        .filter(|i| !i.isHidden() && !i.isAlternate() && !i.isSeparatorItem())
                        .map(|i| i.title().to_string())
                        .collect();
                    let tail = &shown[shown.len().saturating_sub(4)..];
                    out.push(format!(
                        "Edit menu: {} items shown, ending {tail:?}",
                        shown.len()
                    ));
                }
            }
        }
    }
    out.join("; ")
}
