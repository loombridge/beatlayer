//! CLAP, VST3, Audio Unit, and LV2 hosting behind one trait; the scanner; editor windows.
//!
//! - [`clap`]: CLAP hosting (WP-2.6): a plug-in loaded by path runs as a
//!   device in a chain, with sample-accurate note and parameter events in
//!   and out, its state saved with the set, its latency reported, and its
//!   editor in a window of its own.

pub mod clap;
