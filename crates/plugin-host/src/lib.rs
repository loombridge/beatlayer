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
//! - [`scan`]: the plug-in scanner (WP-2.9): finds the plug-ins on this
//!   machine in a child process with a timeout, blocklists the ones that
//!   fail, and caches what it found by path and modification time.

pub mod au;
pub mod clap;
pub mod scan;
