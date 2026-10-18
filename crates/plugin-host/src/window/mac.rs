//! macOS: the host's own window for a plug-in's editor, with the plug-in's
//! view embedded as a subview of its content view (WP-2.10; CLAP's `cocoa`
//! API, which measures in logical points, so the plug-in is never told a
//! scale).
//!
//! The window is a utility panel that stays above the main window, as
//! Live's plug-in windows are (L-OTH-006): a small title bar with a close
//! button, titled `<device>/<track>`. It stays shown while the app isn't
//! the active one. AppKit makes windows only on the main thread, so a
//! window asked for on another thread is refused, and the host falls back
//! to the plug-in's own floating window.
//!
//! The person closing the window orders it out (it isn't released when
//! closed), and [`Window::poll`] reports that as a close: a window that
//! was last shown and is no longer visible, while the app isn't hidden.

use std::cell::Cell;

use clack_extensions::gui::{GuiSize, Window as ClapWindow};
use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly as _};
use objc2_app_kit::{NSApplication, NSBackingStoreType, NSPanel, NSView, NSWindowStyleMask};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

use super::WindowEvent;

pub struct Window {
    panel: Retained<NSPanel>,
    /// The panel's content view, the parent the plug-in's view is added to.
    view: Retained<NSView>,
    mtm: MainThreadMarker,
    /// The content size last reported (or made), in points: a resize by
    /// the person shows up as a difference from it.
    size: Cell<GuiSize>,
    /// Whether the window was last asked to show: one that was, and isn't
    /// visible, was closed by the person.
    shown: Cell<bool>,
    /// A close already reported.
    closed: Cell<bool>,
}

fn points(size: GuiSize) -> NSSize {
    NSSize {
        width: f64::from(size.width),
        height: f64::from(size.height),
    }
}

/// A size in whole points, as CLAP's `cocoa` API gives and takes it.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a window's size in points is small, positive and rounded"
)]
fn whole(size: NSSize) -> GuiSize {
    GuiSize {
        width: size.width.round().max(1.0) as u32,
        height: size.height.round().max(1.0) as u32,
    }
}

impl Window {
    pub fn open(title: &str, size: GuiSize, resizable: bool) -> Result<Self, String> {
        let mtm = MainThreadMarker::new()
            .ok_or_else(|| "a window opens only on the main thread".to_owned())?;
        let mut style = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::UtilityWindow;
        if resizable {
            style |= NSWindowStyleMask::Resizable;
        }
        let rect = NSRect {
            origin: NSPoint { x: 0.0, y: 0.0 },
            size: points(size),
        };
        let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
            NSPanel::alloc(mtm),
            rect,
            style,
            NSBackingStoreType::Buffered,
            false,
        );
        // SAFETY: the panel is released when our own reference goes, not
        // when the person closes it, so a closed panel can still be
        // looked at and dropped.
        unsafe { panel.setReleasedWhenClosed(false) };
        panel.setTitle(&NSString::from_str(title));
        // Above the main window, and shown while the app isn't active, as
        // Live's plug-in windows are.
        panel.setFloatingPanel(true);
        panel.setHidesOnDeactivate(false);
        panel.setBecomesKeyOnlyIfNeeded(true);
        let view = NSView::initWithFrame(NSView::alloc(mtm), rect);
        panel.setContentView(Some(&view));
        panel.center();
        Ok(Self {
            panel,
            view,
            mtm,
            size: Cell::new(size),
            shown: Cell::new(false),
            closed: Cell::new(false),
        })
    }

    pub fn handle(&self) -> ClapWindow<'static, '_> {
        let view: *const NSView = &raw const *self.view;
        // SAFETY: the view lives as long as this window, which outlives the
        // plug-in's GUI (the host destroys the GUI first).
        unsafe { ClapWindow::from_cocoa_nsview(view.cast_mut().cast()) }
    }

    /// The display's backing scale, reported but never told to the
    /// plug-in: CLAP's `cocoa` API measures in points.
    pub fn scale(&self) -> f64 {
        // SAFETY: `backingScaleFactor` takes nothing and returns a CGFloat.
        unsafe { objc2::msg_send![&*self.panel, backingScaleFactor] }
    }

    pub fn size(&self) -> GuiSize {
        self.size.get()
    }

    pub fn resize(&self, size: GuiSize) {
        self.panel.setContentSize(points(size));
        self.size.set(size);
    }

    pub fn set_visible(&self, shown: bool) {
        if shown {
            self.panel.orderFrontRegardless();
        } else {
            self.panel.orderOut(None);
        }
        self.shown.set(shown);
    }

    pub fn poll(&self) -> Option<WindowEvent> {
        if self.closed.get() {
            return None;
        }
        let app_hidden = NSApplication::sharedApplication(self.mtm).isHidden();
        if self.shown.get() && !self.panel.isVisible() && !app_hidden {
            self.closed.set(true);
            return Some(WindowEvent::Closed);
        }
        let now = whole(self.view.frame().size);
        if now != self.size.get() {
            self.size.set(now);
            return Some(WindowEvent::Resized(now));
        }
        None
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        self.panel.orderOut(None);
        self.panel.close();
    }
}
