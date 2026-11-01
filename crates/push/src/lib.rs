//! Push 2 as a control surface: its controls and LEDs, its display, and its
//! modes (`docs/specs/push-2.md`, WP-7.19).
//!
//! The surface is [`Push`], an implementation of `bl-control`'s
//! `ControlSurface`; [`kind`] is what the Control Surfaces table's chooser
//! offers. Each mode's content plugs in through [`content`]. The simulated
//! Push 2 the tests use is [`sim`].

use std::sync::Arc;

use bl_control::{Kind, Ring};

pub mod browse;
pub mod buttons;
pub mod content;
pub mod controls;
pub mod curve;
pub mod device;
pub mod display;
pub mod encoder;
pub mod frame;
pub mod leds;
pub mod mix;
pub mod note;
pub mod paint;
pub mod ports;
pub mod recording;
pub mod screen;
pub mod session;
pub mod sim;
pub mod state;
mod surface;
pub mod sysex;

pub use content::{Contents, MakeContents};
pub use display::Claim;
pub use state::{Command, Handle, Shared, Snapshot};
pub use surface::Push;

/// The surface's ID, as the preferences and `control_surface.set` keep it.
pub const SURFACE_ID: &str = "push-2";

/// The Push 2 surface as the Control Surfaces table's chooser offers it
/// (R2): `shared` is what the preferences keep, `claim` takes the display
/// interface, and `contents` makes the content of each mode.
#[must_use]
pub fn kind(shared: Arc<Shared>, claim: Claim, contents: MakeContents) -> Kind {
    Kind::new(
        SURFACE_ID,
        "Push 2",
        true,
        false,
        Some(Ring {
            tracks: 8,
            scenes: 8,
        }),
        Vec::new(),
        move || {
            Box::new(Push::new(
                Arc::clone(&shared),
                Arc::clone(&claim),
                &contents,
            ))
        },
    )
}

/// The placeholders every mode shows until its package builds its content.
#[must_use]
pub fn placeholder_contents() -> MakeContents {
    Arc::new(Contents::placeholders)
}

/// The contents of the modes that are built: Note Mode's, Device Mode's,
/// Browse Mode's, Mix Mode's and Session Mode's own, and the placeholders for
/// the others (`docs/specs/push-2-notes.md`, `docs/specs/push-2-tracks.md` R5,
/// R15, R28, R59).
#[must_use]
pub fn contents() -> MakeContents {
    Arc::new(|| {
        let mut contents = Contents::placeholders();
        contents.pads[0] = Box::new(note::Note::new());
        contents.display[content::DisplayMode::Device.index()] =
            Box::new(device::DeviceMode::new());
        contents.display[content::DisplayMode::Browse.index()] = Box::new(browse::Browse::new());
        contents.display[content::DisplayMode::Mix.index()] = Box::new(mix::Mix::new());
        contents.pads[1] = Box::new(session::Session::new());
        contents
    })
}
