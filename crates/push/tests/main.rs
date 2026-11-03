//! Push 2's tests: the surface on the simulated device (R36).

// The real-time allocation trap, once for the whole binary: a test file
// can't install its own global allocator.
bl_rt::install_trap!();

mod browse;
mod clip;
mod connect;
mod controls;
mod devices;
mod display;
mod leds;
mod melodies;
mod mix;
mod modes;
mod realtime;
mod rig;
mod session;
mod sim;
mod simpler;
