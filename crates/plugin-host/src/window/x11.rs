//! X11 platforms (Linux and the other X11 Unixes): the host's own X11
//! window for a plug-in's editor, with the plug-in's view embedded as a
//! child of it (WP-2.10; CLAP's `x11` API, which measures in physical
//! pixels). A Wayland session serves it through XWayland, as plug-in
//! editors there draw into X11.
//!
//! The window is a plain top-level one, placed by the window manager. Its
//! scale comes from the display's `Xft.dpi` resource, as other X11
//! toolkits read the display's scaling; it's 1.0 when the display doesn't
//! say. The person closing the window asks for the editor to close through
//! `WM_DELETE_WINDOW`, rather than the window manager killing the
//! connection.

use std::cell::Cell;
use std::os::raw::c_ulong;

use clack_extensions::gui::{GuiSize, Window as ClapWindow};
use x11rb::connection::Connection as _;
use x11rb::properties::WmSizeHints;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    AtomEnum, ConfigureWindowAux, ConnectionExt as _, CreateWindowAux, EventMask, PropMode,
    Window as XWindow, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{COPY_DEPTH_FROM_PARENT, COPY_FROM_PARENT};

#[cfg(test)]
use x11rb::protocol::xproto::ClientMessageEvent;

use super::WindowEvent;

pub struct Window {
    /// The X connection; the display's scale was read when the window
    /// opened.
    conn: RustConnection,
    /// The window itself; dropping this destroys it.
    window: XWindow,
    /// The atoms a `WM_DELETE_WINDOW` message names.
    protocols: u32,
    delete_window: u32,
    /// The client size last reported (or made), in physical pixels: a
    /// resize by the person shows up as a difference from it.
    size: Cell<GuiSize>,
    scale: f64,
    /// What `set_visible` last asked for. A window the person iconified
    /// stays as it is: mapping it again would ask the window manager to
    /// restore it.
    mapped: Cell<bool>,
}

/// Connects to the display. A busy X server can reset a new connection
/// before its setup reply (Xvfb does when many clients connect at once),
/// so a failed connection is tried a few more times, briefly, before the
/// editor is refused. A display that isn't there fails at once.
fn connect() -> Result<(RustConnection, usize), String> {
    const ATTEMPTS: u32 = 5;
    let mut attempt = 1;
    loop {
        match RustConnection::connect(None) {
            Ok(connected) => return Ok(connected),
            Err(x11rb::errors::ConnectError::IoError(error))
                if attempt < ATTEMPTS
                    && matches!(
                        error.kind(),
                        std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::UnexpectedEof
                    ) =>
            {
                std::thread::sleep(std::time::Duration::from_millis(25 << attempt));
                attempt += 1;
            }
            Err(error) => return Err(format!("couldn't reach an X display: {error}")),
        }
    }
}

impl Window {
    pub fn open(title: &str, size: GuiSize, resizable: bool) -> Result<Self, String> {
        let (conn, screen_num) = connect()?;
        let screen = &conn.setup().roots[screen_num];
        let window = conn.generate_id().map_err(why)?;
        let protocols = atom(&conn, b"WM_PROTOCOLS")?;
        let delete_window = atom(&conn, b"WM_DELETE_WINDOW")?;
        let width = u16::try_from(size.width)
            .map_err(|_| format!("a window can't be {} pixels wide", size.width))?;
        let height = u16::try_from(size.height)
            .map_err(|_| format!("a window can't be {} pixels tall", size.height))?;
        // The window is made unmapped and shown once the plug-in's view is
        // embedded in it; the person resizing it is reported by `poll`.
        conn.create_window(
            COPY_DEPTH_FROM_PARENT,
            window,
            screen.root,
            0,
            0,
            width,
            height,
            0,
            WindowClass::INPUT_OUTPUT,
            COPY_FROM_PARENT,
            &CreateWindowAux::new().event_mask(EventMask::STRUCTURE_NOTIFY),
        )
        .map_err(why)?;
        // X11's title; the window manager reads it.
        conn.change_property8(
            PropMode::REPLACE,
            window,
            AtomEnum::WM_NAME,
            AtomEnum::STRING,
            title.as_bytes(),
        )
        .map_err(why)?;
        // A plug-in that takes no new size: one fixed size from the window
        // manager.
        if !resizable {
            let mut hints = WmSizeHints::new();
            hints.min_size = Some((i32::from(width), i32::from(height)));
            hints.max_size = Some((i32::from(width), i32::from(height)));
            hints.set_normal_hints(&conn, window).map_err(why)?;
        }
        // A close from the person is a message for us, not the connection
        // being killed.
        conn.change_property32(
            PropMode::REPLACE,
            window,
            protocols,
            AtomEnum::ATOM,
            &[delete_window],
        )
        .map_err(why)?;
        conn.flush().map_err(why)?;
        let scale = display_scale(&conn, screen.root);
        Ok(Self {
            conn,
            window,
            protocols,
            delete_window,
            size: Cell::new(size),
            scale,
            mapped: Cell::new(false),
        })
    }

    /// The window's X ID, which an LV2 UI embeds its view into.
    pub fn id(&self) -> u32 {
        self.window
    }

    /// The size of the window `child`, which another connection made inside
    /// this one, in physical pixels.
    pub fn child_size(&self, child: u32) -> Option<GuiSize> {
        let geometry = self.conn.get_geometry(child).ok()?.reply().ok()?;
        Some(GuiSize {
            width: u32::from(geometry.width),
            height: u32::from(geometry.height),
        })
    }

    pub fn handle(&self) -> ClapWindow<'static, '_> {
        ClapWindow::from_x11_handle(c_ulong::from(self.window))
    }

    pub fn scale(&self) -> f64 {
        self.scale
    }

    pub fn size(&self) -> GuiSize {
        self.size.get()
    }

    /// Makes the client area `size` physical pixels.
    pub fn resize(&self, size: GuiSize) {
        let _ = self.conn.configure_window(
            self.window,
            &ConfigureWindowAux::new()
                .width(size.width)
                .height(size.height),
        );
        let _ = self.conn.flush();
        self.size.set(size);
    }

    pub fn set_visible(&self, shown: bool) {
        if self.mapped.replace(shown) == shown {
            return;
        }
        let _ = if shown {
            self.conn.map_window(self.window)
        } else {
            self.conn.unmap_window(self.window)
        };
        let _ = self.conn.flush();
    }

    pub fn poll(&self) -> Option<WindowEvent> {
        loop {
            let event = match self.conn.poll_for_event() {
                Ok(Some(event)) => event,
                // Nothing new since the last look.
                Ok(None) => return None,
                // A connection that's gone has no window either.
                Err(_) => return Some(WindowEvent::Closed),
            };
            match event {
                Event::ConfigureNotify(event) if event.window == self.window => {
                    let size = GuiSize {
                        width: u32::from(event.width),
                        height: u32::from(event.height),
                    };
                    if size != self.size.get() {
                        self.size.set(size);
                        return Some(WindowEvent::Resized(size));
                    }
                }
                Event::ClientMessage(event)
                    if event.type_ == self.protocols
                        && event.data.as_data32()[0] == self.delete_window =>
                {
                    return Some(WindowEvent::Closed);
                }
                Event::DestroyNotify(event) if event.window == self.window => {
                    return Some(WindowEvent::Closed);
                }
                _ => {}
            }
        }
    }

    /// For the window layer's tests: the person closed the window, as a
    /// window manager tells it with `WM_DELETE_WINDOW`.
    #[cfg(test)]
    pub fn pretend_closed(&self) {
        let event = ClientMessageEvent::new(
            32,
            self.window,
            self.protocols,
            [self.delete_window, 0, 0, 0, 0],
        );
        let _ = self
            .conn
            .send_event(false, self.window, EventMask::NO_EVENT, event);
        // The message travels with the round trip, so the next look sees
        // it.
        let _ = self
            .conn
            .get_geometry(self.window)
            .map(|cookie| cookie.reply());
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        let _ = self.conn.destroy_window(self.window);
        let _ = self.conn.flush();
    }
}

/// Interns an atom this host asks for by name.
fn atom(conn: &RustConnection, name: &[u8]) -> Result<u32, String> {
    conn.intern_atom(false, name)
        .map_err(why)?
        .reply()
        .map_err(why)
        .map(|reply| reply.atom)
}

/// The display's scale, from `Xft.dpi` in the root window's
/// `RESOURCE_MANAGER` property, as other X11 toolkits read the display's
/// scaling; 1.0 when the display doesn't say (96 DPI, as X11 counts).
fn display_scale(conn: &RustConnection, root: XWindow) -> f64 {
    let Ok(cookie) = conn.get_property(
        false,
        root,
        AtomEnum::RESOURCE_MANAGER,
        AtomEnum::STRING,
        0,
        64 * 1024,
    ) else {
        return 1.0;
    };
    let Ok(reply) = cookie.reply() else {
        return 1.0;
    };
    let manager = String::from_utf8_lossy(&reply.value);
    for line in manager.lines() {
        let Some(dpi) = line
            .strip_prefix("Xft.dpi:")
            .and_then(|rest| rest.trim().parse::<f64>().ok())
        else {
            continue;
        };
        if dpi > 0.0 {
            return dpi / 96.0;
        }
    }
    1.0
}

/// A failed request, as a string for the layer's errors.
fn why(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use x11rb::connection::Connection as _;
    use x11rb::protocol::Event;
    use x11rb::protocol::xproto::{ChangeWindowAttributesAux, ConnectionExt as _, EventMask};
    use x11rb::rust_connection::RustConnection;

    use super::{GuiSize, Window};

    // Spec: lv2-hosting R13. Showing a window that is already shown asks the
    // window manager for nothing: it would restore an iconified editor.
    #[test]
    fn showing_a_shown_window_again_maps_it_once() {
        let (manager, screen) = RustConnection::connect(None).expect("a display");
        let root = manager.setup().roots[screen].root;
        // This client plays the window manager: a map request from another
        // client reaches it, and the window stays unmapped.
        manager
            .change_window_attributes(
                root,
                &ChangeWindowAttributesAux::new().event_mask(EventMask::SUBSTRUCTURE_REDIRECT),
            )
            .unwrap();
        manager.flush().unwrap();
        let window = Window::open(
            "t",
            GuiSize {
                width: 100,
                height: 100,
            },
            true,
        )
        .expect("a window");
        window.set_visible(true);
        window.set_visible(true);
        window.set_visible(true);
        // A round trip on each connection brings the requests in order.
        window
            .conn
            .get_geometry(window.window)
            .unwrap()
            .reply()
            .unwrap();
        manager.get_input_focus().unwrap().reply().unwrap();
        let mut requests = 0;
        while let Some(event) = manager.poll_for_event().unwrap() {
            if matches!(event, Event::MapRequest(e) if e.window == window.id()) {
                requests += 1;
            }
        }
        assert_eq!(requests, 1);
    }
}
