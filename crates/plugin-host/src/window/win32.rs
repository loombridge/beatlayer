//! Windows: the host's own HWND for a plug-in's editor, with the plug-in's
//! view embedded as a child of it (WP-2.10; CLAP's `win32` API, which
//! measures in physical pixels).
//!
//! The window is a plain top-level one: the system's frame and title bar,
//! resizable by the person. Its scale is the DPI of the monitor it's on
//! (`GetDpiForWindow`), read again on every look, so a window dragged to a
//! monitor of another DPI is noticed; `WM_DPICHANGED` resizes the window to
//! the rect Windows suggests for the new DPI, as per-monitor-DPI apps must.

use std::cell::Cell;
use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt as _;
use std::ptr;
use std::sync::Once;

use clack_extensions::gui::{GuiSize, Window as ClapWindow};
use windows_sys::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::COLOR_WINDOW;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_USERDATA, GetClientRect,
    GetWindowLongPtrW, IsWindow, RegisterClassExW, SW_HIDE, SW_SHOW, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOZORDER, SetWindowLongPtrW, SetWindowPos, ShowWindow, WINDOW_STYLE, WM_CLOSE,
    WM_DPICHANGED, WNDCLASSEXW, WS_MAXIMIZEBOX, WS_OVERLAPPEDWINDOW, WS_THICKFRAME,
};

use super::WindowEvent;

/// The window class every editor window uses: one per process, registered
/// the first time a window opens.
const CLASS_NAME: &str = "BeatlayerPlugInEditor";

static CLASS: Once = Once::new();

pub struct Window {
    /// The window itself; dropping this destroys it.
    hwnd: HWND,
    /// The style it was made with: `resize` measures the frame the same
    /// way.
    style: WINDOW_STYLE,
    /// The client size last reported (or made), in physical pixels: a
    /// resize by the person shows up as a difference from it.
    size: Cell<GuiSize>,
    /// The display's scale last reported: a move to a monitor of another
    /// DPI shows up as a difference from it.
    scale: Cell<f64>,
}

impl Window {
    pub fn open(title: &str, size: GuiSize, resizable: bool) -> Result<Self, String> {
        // SAFETY: a null module name asks for this process's own module.
        let hinstance = unsafe { GetModuleHandleW(ptr::null()) };
        register_class(hinstance);
        let class = wide(CLASS_NAME);
        let title = wide(title);
        // A plug-in that takes no new size gets no frame to drag.
        let style = if resizable {
            WS_OVERLAPPEDWINDOW
        } else {
            WS_OVERLAPPEDWINDOW & !(WS_MAXIMIZEBOX | WS_THICKFRAME)
        };
        // The window is made hidden and shown once the plug-in's view is
        // embedded in it. The size it opens at is made right below, once
        // its monitor's DPI is known.
        // SAFETY: the class is registered for this module; the class name
        // and title live for the call; nothing is passed as `lpParam`.
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                title.as_ptr(),
                style,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                size.width as i32,
                size.height as i32,
                0,
                0,
                hinstance,
                ptr::null(),
            )
        };
        if hwnd == 0 {
            return Err(format!(
                "couldn't open a window: {}",
                std::io::Error::last_os_error()
            ));
        }
        let window = Self {
            hwnd,
            style,
            size: Cell::new(size),
            scale: Cell::new(dpi_scale(hwnd)),
        };
        // The size asked for is the client area; the frame takes its room
        // out of the window's.
        window.resize(size);
        Ok(window)
    }

    pub fn handle(&self) -> ClapWindow<'static, '_> {
        // SAFETY: the handle is this window's, and outlives its use: the
        // plug-in's GUI is destroyed before the window is.
        unsafe { ClapWindow::from_win32_hwnd(self.hwnd as *mut c_void) }
    }

    pub fn scale(&self) -> f64 {
        self.scale.get()
    }

    pub fn size(&self) -> GuiSize {
        self.size.get()
    }

    /// Makes the client area `size` physical pixels.
    pub fn resize(&self, size: GuiSize) {
        let dpi = (self.scale.get() * 96.0).round() as u32;
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: size.width as i32,
            bottom: size.height as i32,
        };
        // SAFETY: `rect` is a live RECT, and the style is this window's.
        unsafe { AdjustWindowRectExForDpi(&mut rect, self.style, 0, 0, dpi) };
        // SAFETY: `hwnd` is this window, used on its own thread; the rect
        // is the frame the client area needs.
        unsafe {
            SetWindowPos(
                self.hwnd,
                0,
                0,
                0,
                rect.right - rect.left,
                rect.bottom - rect.top,
                SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOZORDER,
            );
        }
        self.size.set(size);
    }

    pub fn set_visible(&self, shown: bool) {
        // SAFETY: `hwnd` is this window, used on its own thread.
        unsafe { ShowWindow(self.hwnd, if shown { SW_SHOW } else { SW_HIDE }) };
    }

    pub fn poll(&self) -> Option<WindowEvent> {
        // SAFETY: `hwnd` is this window; a destroyed one is what `IsWindow`
        // reports.
        if unsafe { IsWindow(self.hwnd) } == 0 {
            return Some(WindowEvent::Closed);
        }
        // SAFETY: as above; the window procedure marks a close in the user
        // data.
        if unsafe { GetWindowLongPtrW(self.hwnd, GWLP_USERDATA) } != 0 {
            return Some(WindowEvent::Closed);
        }
        let scale = dpi_scale(self.hwnd);
        if (scale - self.scale.get()).abs() > 1e-6 {
            self.scale.set(scale);
            return Some(WindowEvent::ScaleChanged(scale));
        }
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        // SAFETY: `rect` is a live RECT, and `hwnd` is this window.
        if unsafe { GetClientRect(self.hwnd, &mut rect) } == 0 {
            return None;
        }
        let size = GuiSize {
            #[expect(clippy::cast_sign_loss, reason = "a client rect is never negative")]
            width: (rect.right - rect.left) as u32,
            #[expect(clippy::cast_sign_loss, reason = "a client rect is never negative")]
            height: (rect.bottom - rect.top) as u32,
        };
        if size != self.size.get() {
            self.size.set(size);
            return Some(WindowEvent::Resized(size));
        }
        None
    }

    /// For the window layer's tests: the person closed the window, as
    /// Windows' close button does.
    #[cfg(test)]
    pub fn pretend_closed(&self) {
        use windows_sys::Win32::UI::WindowsAndMessaging::SendMessageW;
        // SAFETY: `hwnd` is this window, used on its own thread; the
        // message goes through the window procedure.
        unsafe { SendMessageW(self.hwnd, WM_CLOSE, 0, 0) };
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        // SAFETY: `hwnd` is this window, destroyed once.
        unsafe { DestroyWindow(self.hwnd) };
    }
}

/// Registers the editor windows' class once per process; a class that's
/// already there is the one we want.
fn register_class(hinstance: HINSTANCE) {
    CLASS.call_once(|| {
        let name = wide(CLASS_NAME);
        let class = WNDCLASSEXW {
            cbSize: u32::try_from(std::mem::size_of::<WNDCLASSEXW>()).unwrap_or(u32::MAX),
            style: 0,
            lpfnWndProc: Some(window_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinstance,
            hIcon: 0,
            hCursor: 0,
            lpszMenuName: ptr::null(),
            lpszClassName: name.as_ptr(),
            hIconSm: 0,
            // The system's window colour, behind the plug-in's view.
            hbrBackground: (COLOR_WINDOW + 1) as isize,
        };
        // SAFETY: the class is filled in with this module's instance and
        // its procedure; the class keeps its own copy of the name.
        unsafe { RegisterClassExW(&class) };
    });
}

/// The window procedure: a window moved to a monitor of another DPI is
/// resized to the rect Windows suggests for it. The close button hides the
/// window and marks it closed for `poll`; the window is destroyed only when
/// it drops, after the plug-in's view in it is. Everything else is the
/// default procedure's.
///
/// # Safety
///
/// Windows calls this with a live window and the message's own arguments.
unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    _wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_CLOSE {
        // SAFETY: `hwnd` is the window this message is for, used on its own
        // thread. The default procedure would destroy it, and with it the
        // plug-in's child window, before the plug-in's GUI is destroyed.
        unsafe {
            ShowWindow(hwnd, SW_HIDE);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 1);
        }
        return 0;
    }
    if message == WM_DPICHANGED {
        // SAFETY: with WM_DPICHANGED, `lparam` points to the suggested
        // window rect, in physical pixels, live for the call.
        let rect = unsafe { &*(lparam as *const RECT) };
        // SAFETY: `hwnd` is the window this message is for, used on its own
        // thread; the rect is Windows' own suggestion.
        unsafe {
            SetWindowPos(
                hwnd,
                0,
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                SWP_NOACTIVATE | SWP_NOZORDER,
            );
        }
        return 0;
    }
    // SAFETY: a window procedure is called with a live window and its
    // message's own arguments.
    unsafe { DefWindowProcW(hwnd, message, _wparam, lparam) }
}

/// The scale of the monitor `hwnd` is on: 96 DPI is 1.0, as Windows counts.
fn dpi_scale(hwnd: HWND) -> f64 {
    // SAFETY: `hwnd` is this process's window.
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    if dpi == 0 { 1.0 } else { f64::from(dpi) / 96.0 }
}

/// `text` as a NUL-terminated UTF-16 string for the Win32 calls.
fn wide(text: &str) -> Vec<u16> {
    std::ffi::OsStr::new(text)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Spec: clap-hosting R13. The close button doesn't destroy the window
    /// under the plug-in's GUI: the host reports the close, and the window
    /// is destroyed once, when the host drops it.
    #[test]
    fn the_close_button_leaves_the_window_alive_for_the_plugin_to_go_first() {
        let window = Window::open(
            "Test Gain Echo/1-Audio",
            GuiSize {
                width: 240,
                height: 120,
            },
            true,
        )
        .expect("a window");
        window.pretend_closed();
        assert_eq!(window.poll(), Some(WindowEvent::Closed));
        // SAFETY: `hwnd` is this window.
        assert_ne!(unsafe { IsWindow(window.hwnd) }, 0, "still a window");
    }
}
