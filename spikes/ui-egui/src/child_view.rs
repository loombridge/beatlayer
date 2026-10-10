//! The plug-in editor stand-in: a foreign child view inside a host window.
//!
//! A real plug-in receives its host window's native handle and attaches its
//! own view to it. This module does the same from the winit window's
//! `raw-window-handle`: an `NSView` on macOS, a child `HWND` on Windows, and
//! a child X11 window (through its own X connection, as a plug-in would) on
//! Linux. Wayland has no child windows, so Linux hosts run plug-in editors
//! under XWayland.

use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// Where the child goes inside the host window, in logical points.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// The fill colour, sRGB 0 to 1.
pub const COLOUR: [f64; 3] = [0.96, 0.55, 0.12];

/// An embedded child view; dropping it removes it from its parent.
pub struct ChildView {
    #[allow(dead_code)] // Held for its Drop.
    inner: Inner,
    pub description: String,
}

/// Embeds a solid-colour child view in `window`.
pub fn embed(
    window: &impl HasWindowHandle,
    scale_factor: f64,
    frame: Frame,
) -> Result<ChildView, String> {
    let handle = window
        .window_handle()
        .map_err(|e| format!("no window handle: {e}"))?
        .as_raw();
    match handle {
        #[cfg(target_os = "macos")]
        RawWindowHandle::AppKit(h) => {
            let _ = scale_factor; // AppKit works in points.
            macos::embed(h, frame)
        }
        #[cfg(target_os = "windows")]
        RawWindowHandle::Win32(h) => windows::embed(h, frame, scale_factor),
        #[cfg(all(unix, not(target_os = "macos")))]
        RawWindowHandle::Xlib(h) => x11::embed(h.window as u32, frame, scale_factor),
        #[cfg(all(unix, not(target_os = "macos")))]
        RawWindowHandle::Xcb(h) => x11::embed(h.window.get(), frame, scale_factor),
        RawWindowHandle::Wayland(_) => Err(
            "Wayland has no child windows; run with --x11 so the editor window uses XWayland"
                .to_owned(),
        ),
        other => Err(format!("unsupported window handle: {other:?}")),
    }
}

#[cfg(target_os = "macos")]
use macos::Inner;
#[cfg(target_os = "windows")]
use windows::Inner;
#[cfg(all(unix, not(target_os = "macos")))]
use x11::Inner;

#[cfg(target_os = "macos")]
mod macos {
    use objc2::rc::Retained;
    use objc2::{MainThreadMarker, MainThreadOnly as _};
    use objc2_app_kit::NSView;
    use objc2_core_graphics::CGColor;
    use objc2_foundation::{NSPoint, NSRect, NSSize};
    use raw_window_handle::AppKitWindowHandle;

    use super::{COLOUR, ChildView, Frame};

    pub struct Inner {
        view: Retained<NSView>,
    }

    impl Drop for Inner {
        fn drop(&mut self) {
            self.view.removeFromSuperview();
        }
    }

    pub fn embed(handle: AppKitWindowHandle, frame: Frame) -> Result<ChildView, String> {
        let mtm =
            MainThreadMarker::new().ok_or("child views must be created on the main thread")?;
        // SAFETY: winit's AppKit handle points at the window's content view,
        // which lives as long as the window, and we're on the main thread.
        let parent: &NSView = unsafe { handle.ns_view.cast::<NSView>().as_ref() };

        let rect = NSRect::new(
            NSPoint::new(frame.x, frame.y),
            NSSize::new(frame.width, frame.height),
        );
        let view = NSView::initWithFrame(NSView::alloc(mtm), rect);
        view.setWantsLayer(true);
        let layer = view.layer().ok_or("the child view has no layer")?;
        let colour = CGColor::new_srgb(COLOUR[0], COLOUR[1], COLOUR[2], 1.0);
        layer.setBackgroundColor(Some(&colour));
        parent.addSubview(&view);

        let subviews = parent.subviews().len();
        let window_number = parent.window().map_or(-1, |w| w.windowNumber());
        Ok(ChildView {
            description: format!(
                "NSView {:p} added to content view {:p} of window number {window_number} \
                 ({subviews} subviews), frame {}x{} at ({}, {})",
                &*view, parent, frame.width, frame.height, frame.x, frame.y
            ),
            inner: Inner { view },
        })
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use raw_window_handle::Win32WindowHandle;
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::Graphics::Gdi::CreateSolidBrush;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, WNDCLASSW, WS_CHILD,
        WS_VISIBLE,
    };

    use super::{COLOUR, ChildView, Frame};

    pub struct Inner {
        hwnd: HWND,
    }

    impl Drop for Inner {
        fn drop(&mut self) {
            // SAFETY: `hwnd` is the child window this module created.
            unsafe { DestroyWindow(self.hwnd) };
        }
    }

    pub fn embed(handle: Win32WindowHandle, frame: Frame, scale: f64) -> Result<ChildView, String> {
        let parent = handle.hwnd.get() as HWND;
        let class: Vec<u16> = "BeatlayerSpikeChild\0".encode_utf16().collect();
        let [r, g, b] = COLOUR.map(|c| (c * 255.0).round() as u32);
        // SAFETY: plain Win32 calls with valid, NUL-terminated strings; the
        // parent HWND comes from winit and outlives the child.
        let hwnd = unsafe {
            let instance = GetModuleHandleW(std::ptr::null());
            let class_info = WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(DefWindowProcW),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: instance,
                hIcon: std::ptr::null_mut(),
                hCursor: std::ptr::null_mut(),
                hbrBackground: CreateSolidBrush(r | (g << 8) | (b << 16)),
                lpszMenuName: std::ptr::null(),
                lpszClassName: class.as_ptr(),
            };
            // A second registration fails harmlessly with "class exists".
            RegisterClassW(&class_info);
            CreateWindowExW(
                0,
                class.as_ptr(),
                std::ptr::null(),
                WS_CHILD | WS_VISIBLE,
                (frame.x * scale) as i32,
                (frame.y * scale) as i32,
                (frame.width * scale) as i32,
                (frame.height * scale) as i32,
                parent,
                std::ptr::null_mut(),
                instance,
                std::ptr::null(),
            )
        };
        if hwnd.is_null() {
            return Err("CreateWindowExW failed".to_owned());
        }
        Ok(ChildView {
            description: format!("child HWND {hwnd:p} in parent {parent:p}"),
            inner: Inner { hwnd },
        })
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod x11 {
    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::{ConnectionExt as _, CreateWindowAux, WindowClass};
    use x11rb::rust_connection::RustConnection;

    use super::{COLOUR, ChildView, Frame};

    pub struct Inner {
        conn: RustConnection,
        window: u32,
    }

    impl Drop for Inner {
        fn drop(&mut self) {
            let _ = self.conn.destroy_window(self.window);
            let _ = self.conn.flush();
        }
    }

    pub fn embed(parent: u32, frame: Frame, scale: f64) -> Result<ChildView, String> {
        let (conn, _screen) = x11rb::connect(None).map_err(|e| e.to_string())?;
        let window = conn.generate_id().map_err(|e| e.to_string())?;
        let [r, g, b] = COLOUR.map(|c| (c * 255.0).round() as u32);
        conn.create_window(
            x11rb::COPY_DEPTH_FROM_PARENT,
            window,
            parent,
            (frame.x * scale) as i16,
            (frame.y * scale) as i16,
            (frame.width * scale) as u16,
            (frame.height * scale) as u16,
            0,
            WindowClass::INPUT_OUTPUT,
            x11rb::COPY_FROM_PARENT,
            &CreateWindowAux::new().background_pixel((r << 16) | (g << 8) | b),
        )
        .map_err(|e| e.to_string())?;
        conn.map_window(window).map_err(|e| e.to_string())?;
        conn.flush().map_err(|e| e.to_string())?;
        Ok(ChildView {
            description: format!("X11 window {window:#x} in parent {parent:#x}"),
            inner: Inner { conn, window },
        })
    }
}
