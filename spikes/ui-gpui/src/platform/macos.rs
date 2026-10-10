//! macOS pieces GPUI doesn't provide: pointer lock, and a native child view
//! standing in for a plug-in editor.

use std::cell::{Cell, RefCell};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicUsize, Ordering};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSColor, NSCursor, NSEvent, NSEventMask, NSEventType, NSRectFill,
    NSResponder, NSView,
};
use objc2_foundation::{NSObject, NSPoint, NSRect, NSSize};
use raw_window_handle::RawWindowHandle;

/// An opaque `CGEventRef` target, encoded as Objective-C expects.
#[repr(C)]
pub struct CGEventOpaque([u8; 0]);

// SAFETY: the encoding matches CoreGraphics' `struct __CGEvent *`.
unsafe impl objc2::encode::RefEncode for CGEventOpaque {
    const ENCODING_REF: objc2::encode::Encoding =
        objc2::encode::Encoding::Pointer(&objc2::encode::Encoding::Struct("__CGEvent", &[]));
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

// CoreGraphics. `boolean_t` is a 32-bit unsigned int on Apple platforms.
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGAssociateMouseAndMouseCursorPosition(connected: u32) -> i32;
    fn CGEventCreateMouseEvent(
        source: *const std::ffi::c_void,
        kind: u32,
        location: CGPoint,
        button: u32,
    ) -> *mut CGEventOpaque;
    fn CGEventSetIntegerValueField(event: *mut CGEventOpaque, field: u32, value: i64);
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: *const std::ffi::c_void);
}

/// Posts left-button drag events carrying the given motion deltas to this
/// app's own event queue (`NSApp.postEvent`), so they reach the pointer-lock
/// monitor exactly as hardware drags do. No Accessibility permission is
/// needed, because nothing is posted outside the process. Used by
/// `--selftest`.
pub fn post_drag_deltas(deltas: &[(f64, f64)]) -> usize {
    const LEFT_MOUSE_DRAGGED: u32 = 6;
    const DELTA_X: u32 = 4;
    const DELTA_Y: u32 = 5;
    let Some(mtm) = MainThreadMarker::new() else {
        return 0;
    };
    let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
    let mut posted = 0;
    for &(dx, dy) in deltas {
        // SAFETY: CoreGraphics returns an owned event or null; we release it
        // after AppKit has wrapped (and retained) it.
        unsafe {
            let ev = CGEventCreateMouseEvent(
                std::ptr::null(),
                LEFT_MOUSE_DRAGGED,
                CGPoint { x: 0.0, y: 0.0 },
                0,
            );
            if ev.is_null() {
                continue;
            }
            CGEventSetIntegerValueField(ev, DELTA_X, dx as i64);
            CGEventSetIntegerValueField(ev, DELTA_Y, dy as i64);
            let ns: Option<Retained<NSEvent>> =
                msg_send![<NSEvent as objc2::ClassType>::class(), eventWithCGEvent: ev];
            CFRelease(ev.cast());
            if let Some(ns) = ns {
                app.postEvent_atStart(&ns, false);
                posted += 1;
            }
        }
    }
    posted
}

/// Makes the window transparent to real mouse input (or not), so a scripted
/// test isn't disturbed by someone using the mouse at the same time.
pub fn set_ignores_mouse_events(handle: RawWindowHandle, ignore: bool) {
    let RawWindowHandle::AppKit(handle) = handle else {
        return;
    };
    // SAFETY: GPUI's content NSView is alive while the window exists, and
    // this runs on the main thread.
    let view: &NSView = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
    if let Some(window) = view.window() {
        window.setIgnoresMouseEvents(ignore);
    }
}

/// Posts a left click at `(x, y)` (window coordinates, origin bottom left)
/// to window `window_number` through AppKit's own queue, so AppKit hit-tests
/// it to whichever view is there, as it would a real click. Used by
/// `--selftest` to check that the embedded child view gets mouse input.
pub fn post_click(window_number: isize, x: f64, y: f64) -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
    let mut ok = true;
    for kind in [NSEventType::LeftMouseDown, NSEventType::LeftMouseUp] {
        let ev = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
            kind,
            NSPoint::new(x, y),
            objc2_app_kit::NSEventModifierFlags::empty(),
            0.0,
            window_number,
            None,
            0,
            1,
            if kind == NSEventType::LeftMouseDown { 1.0 } else { 0.0 },
        );
        match ev {
            Some(ev) => app.postEvent_atStart(&ev, false),
            None => ok = false,
        }
    }
    ok
}

/// How many mouse-down events the embedded child views have received.
pub fn child_view_clicks() -> usize {
    CHILD_CLICKS.load(Ordering::Relaxed)
}

/// The window number of the last plug-in window that got a child view.
pub static PLUGIN_WINDOW: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

/// How many times the embedded child views have drawn.
pub fn child_view_draws() -> usize {
    CHILD_DRAWS.load(Ordering::Relaxed)
}

thread_local! {
    static MONITOR: RefCell<Option<Retained<AnyObject>>> = const { RefCell::new(None) };
    static DELTA: Cell<(f64, f64)> = const { Cell::new((0.0, 0.0)) };
}

/// Locks the pointer: the cursor is hidden and frozen in place, and motion
/// keeps arriving as deltas with no screen edge to stop it.
///
/// AppKit has no single pointer-lock call. This detaches the cursor from the
/// mouse (`CGAssociateMouseAndMouseCursorPosition(false)`), hides it
/// (`NSCursor.hide`), and installs a local event monitor that adds up each
/// drag event's `deltaX`/`deltaY`. GPUI's own mouse-move events keep coming
/// (with an unchanged position), so its handler calls [`take_delta`].
pub fn lock_pointer() -> bool {
    if MONITOR.with(|m| m.borrow().is_some()) {
        return true;
    }
    DELTA.with(|d| d.set((0.0, 0.0)));
    let block = RcBlock::new(|event: NonNull<NSEvent>| -> *mut NSEvent {
        // SAFETY: AppKit passes a valid event for the duration of the call.
        let ev = unsafe { event.as_ref() };
        let kind = ev.r#type();
        if kind == NSEventType::LeftMouseDragged || kind == NSEventType::MouseMoved {
            let (dx, dy) = (ev.deltaX(), ev.deltaY());
            DELTA.with(|d| {
                let (x, y) = d.get();
                d.set((x + dx, y + dy));
            });
        }
        event.as_ptr()
    });
    // SAFETY: the block returns the event it was given, which is valid.
    let monitor = unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(
            NSEventMask::LeftMouseDragged | NSEventMask::MouseMoved,
            &block,
        )
    };
    let Some(monitor) = monitor else {
        return false;
    };
    MONITOR.with(|m| *m.borrow_mut() = Some(monitor));
    // SAFETY: a plain CoreGraphics call with no pointer arguments.
    unsafe { CGAssociateMouseAndMouseCursorPosition(0) };
    NSCursor::hide();
    true
}

/// Takes the motion accumulated since the last call, in points, with y
/// growing downwards.
pub fn take_delta() -> (f64, f64) {
    DELTA.with(|d| d.replace((0.0, 0.0)))
}

pub fn unlock_pointer() {
    let Some(monitor) = MONITOR.with(|m| m.borrow_mut().take()) else {
        return;
    };
    // SAFETY: `monitor` came from `addLocalMonitorForEventsMatchingMask`.
    unsafe { NSEvent::removeMonitor(&monitor) };
    // SAFETY: a plain CoreGraphics call with no pointer arguments.
    unsafe { CGAssociateMouseAndMouseCursorPosition(1) };
    NSCursor::unhide();
}

pub fn pointer_locked() -> bool {
    MONITOR.with(|m| m.borrow().is_some())
}

/// The window server's id for the window behind `handle` (the CGWindowID
/// that screenshot and accessibility tools ask for).
pub fn window_number(handle: RawWindowHandle) -> Option<isize> {
    let RawWindowHandle::AppKit(handle) = handle else {
        return None;
    };
    // SAFETY: GPUI's content NSView is alive while the window exists, and
    // this runs on the main thread.
    let view: &NSView = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
    view.window().map(|w| w.windowNumber())
}

/// Walks the window's platform accessibility tree (the `NSAccessibility`
/// objects AccessKit's macOS adapter publishes, which is what VoiceOver
/// reads through the AX server) and returns it as indented text: role,
/// label, title, and value per node.
///
/// This runs inside the app, so it needs no Accessibility permission. The
/// first walk also activates AccessKit, which builds the tree lazily.
pub fn dump_accessibility(handle: RawWindowHandle, max_nodes: usize) -> String {
    use std::fmt::Write as _;

    use objc2_foundation::{NSArray, NSString};

    fn text(obj: &AnyObject, sel: &str) -> Option<String> {
        // SAFETY: each selector here is an NSAccessibility getter returning an
        // object or nil; `respondsToSelector:` guards optional ones.
        unsafe {
            let sel = objc2::runtime::Sel::register(&std::ffi::CString::new(sel).ok()?);
            let responds: bool = msg_send![obj, respondsToSelector: sel];
            if !responds {
                return None;
            }
            let value: Option<Retained<AnyObject>> = msg_send![obj, performSelector: sel];
            let value = value?;
            let desc: Retained<NSString> = msg_send![&*value, description];
            let s = desc.to_string();
            (!s.is_empty()).then_some(s)
        }
    }

    fn walk(obj: &AnyObject, depth: usize, out: &mut String, count: &mut usize, max: usize) {
        if *count >= max {
            return;
        }
        *count += 1;
        let mut line = format!(
            "{:indent$}{}",
            "",
            text(obj, "accessibilityRole").unwrap_or_default(),
            indent = depth * 2
        );
        for (key, sel) in [
            ("label", "accessibilityLabel"),
            ("title", "accessibilityTitle"),
            ("value", "accessibilityValue"),
        ] {
            if let Some(v) = text(obj, sel) {
                let _ = write!(line, " {key}={v:?}");
            }
        }
        let _ = writeln!(out, "{line}");
        // Some AppKit proxies (titlebar cells) don't implement it.
        // SAFETY: `respondsToSelector:` is NSObject's; `accessibilityChildren`
        // returns an NSArray of elements or nil.
        let children: Option<Retained<NSArray<AnyObject>>> = unsafe {
            let responds: bool =
                msg_send![obj, respondsToSelector: objc2::sel!(accessibilityChildren)];
            if responds {
                msg_send![obj, accessibilityChildren]
            } else {
                None
            }
        };
        if let Some(children) = children {
            for child in children.iter() {
                walk(&child, depth + 1, out, count, max);
            }
        }
    }

    let RawWindowHandle::AppKit(handle) = handle else {
        return String::from("not an AppKit window\n");
    };
    // SAFETY: GPUI's content NSView is alive while the window exists, and
    // this runs on the main thread.
    let view: &NSView = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
    let Some(window) = view.window() else {
        return String::from("view has no window\n");
    };
    let mut out = String::new();
    let mut count = 0;
    let root: &AnyObject = &window;
    walk(root, 0, &mut out, &mut count, max_nodes);
    let _ = writeln!(
        out,
        "({count} nodes{})",
        if count >= max_nodes {
            ", truncated"
        } else {
            ""
        }
    );
    out
}

static CHILD_CLICKS: AtomicUsize = AtomicUsize::new(0);
static CHILD_DRAWS: AtomicUsize = AtomicUsize::new(0);

define_class!(
    // SAFETY: NSView has no subclassing requirements beyond the main thread,
    // and this class adds no ivars and overrides no `dealloc`.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "BeatlayerSpikePluginView"]
    struct PluginView;

    impl PluginView {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            NSColor::colorWithSRGBRed_green_blue_alpha(0.16, 0.55, 0.38, 1.0).setFill();
            NSRectFill(self.bounds());
            if CHILD_DRAWS.fetch_add(1, Ordering::Relaxed) == 0 {
                let b = self.bounds();
                eprintln!(
                    "plug-in child view: first drawRect, {}x{} pt, in window: {}",
                    b.size.width,
                    b.size.height,
                    self.window().is_some()
                );
            }
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, _event: &NSEvent) {
            let n = CHILD_CLICKS.fetch_add(1, Ordering::Relaxed) + 1;
            eprintln!("plug-in child view: mouse down #{n}");
        }
    }
);

/// Adds a solid-coloured native child view to the window behind `handle`,
/// covering everything below a strip of `top_inset` points that GPUI keeps
/// drawing. This is what a plug-in does with the parent handle a host gives
/// it (CLAP `gui.set_parent`, VST3 `IPlugView::attached`).
pub fn embed_child_view(handle: RawWindowHandle, top_inset: f64) -> Result<String, String> {
    let RawWindowHandle::AppKit(handle) = handle else {
        return Err(format!("expected an AppKit handle, got {handle:?}"));
    };
    let mtm = MainThreadMarker::new().ok_or("not on the main thread")?;
    // SAFETY: GPUI hands out its live content NSView; we only borrow it here,
    // on the main thread, and the view outlives this call.
    let parent: &NSView = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
    let bounds = parent.bounds();
    let height = (bounds.size.height - top_inset).max(0.0);
    let y = if parent.isFlipped() { top_inset } else { 0.0 };
    let frame = NSRect::new(NSPoint::new(0.0, y), NSSize::new(bounds.size.width, height));
    // SAFETY: `initWithFrame:` is NSView's designated initialiser.
    let child: Retained<PluginView> =
        unsafe { msg_send![PluginView::alloc(mtm), initWithFrame: frame] };
    child.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    parent.addSubview(&child);
    if let Some(window) = parent.window() {
        PLUGIN_WINDOW.store(window.windowNumber(), Ordering::Relaxed);
    }
    Ok(format!(
        "NSView {:p} added to GPUI's NSView {:p} (flipped: {}), frame {}x{} at y {}",
        Retained::as_ptr(&child),
        handle.ns_view.as_ptr(),
        parent.isFlipped(),
        frame.size.width,
        frame.size.height,
        frame.origin.y
    ))
}
