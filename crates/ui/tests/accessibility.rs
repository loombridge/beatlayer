//! The keyboard and screen-reader baseline for every M1 view (WP-1.14),
//! headless: what the accessibility tree gives a screen reader for tracks,
//! clips, and faders; where the keyboard focus goes and what the tree
//! reports as focused as it moves; Tab, the neighbour keys, and the Navigate
//! menu; Outline View in Focus; Display & Input's keyboard settings; and a
//! walk of the whole command registry that fails for any command with no
//! keyboard path that isn't listed with its reason.
//!
//! Spec: accessibility R1 to R9.

mod helpers;
mod menus;
mod outline_view;
mod reachability;
mod screen_reader;
mod settings;
mod tab_order;
