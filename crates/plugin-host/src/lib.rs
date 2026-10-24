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
//! - [`lv2`]: LV2 hosting, part 1 (WP-5.39): Beatlayer's own host on the
//!   system's lilv library, Linux only; a plug-in instantiated, run block by
//!   block with audio, control and MIDI ports, and its state saved.
//! - [`Hosts`] and [`Loans`]: one offline-render loan (WP-2.6b, WP-4.21)
//!   across the three formats.
//! - [`scan`]: the plug-in scanner (WP-2.9): finds the plug-ins on this
//!   machine in a child process with a timeout, blocklists the ones that
//!   fail, and caches what it found by path and modification time.
//! - [`window`]: host windows for plug-in editors (WP-2.10), an HWND on
//!   Windows, an AppKit utility panel on macOS, and an X11 window
//!   elsewhere, with the editor's view embedded as a child.

pub mod au;
pub mod clap;
mod drive;
mod lend;
pub mod lv2;
pub mod scan;
pub mod vst3;
pub mod window;

pub use lend::{Hosts, Loans};
