//! CLAP, VST3, Audio Unit, and LV2 hosting behind one trait; the scanner; editor windows.
//!
//! - [`clap`]: CLAP hosting (WP-2.6): a plug-in loaded by path runs as a
//!   device in a chain, with sample-accurate note and parameter events in
//!   and out, its state saved with the set, its latency reported, and its
//!   editor in a window of its own.
//! - [`au`]: Audio Unit hosting (WP-2.8), macOS only: a unit found by its
//!   component description runs as a device in a chain, with its
//!   parameters and notes on their frames, its state saved with the set,
//!   and its latency reported. On Windows and Linux the module is a stub
//!   whose commands say Audio Units aren't available.
//! - [`vst3`]: VST3 hosting, part 1 (WP-2.7): a plug-in bundle or module
//!   loaded by path runs as a device, its component and controller
//!   connected, with sample-accurate parameter changes and notes.
//! - [`Hosts`] and [`Loans`]: one offline-render loan (WP-2.6b, WP-4.21)
//!   across the three formats.
//! - [`scan`]: the plug-in scanner (WP-2.9): finds the plug-ins on this
//!   machine in a child process with a timeout, blocklists the ones that
//!   fail, and caches what it found by path and modification time.
//! - [`window`]: host windows for plug-in editors (WP-2.10), an HWND on
//!   Windows and an X11 window elsewhere but macOS, with the editor's view
//!   embedded as a child.

pub mod au;
pub mod clap;
mod lend;
pub mod scan;
pub mod vst3;
#[cfg(any(target_os = "windows", all(unix, not(target_os = "macos"))))]
pub mod window;

pub use lend::{Hosts, Loans};
