//! Each platform's way of reading and raising thread priority, and of
//! handling workgroups (macOS only). Every module provides the same items:
//! `HeldPriority`, `current_priority`, `promote`, and the workgroup calls.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub(crate) use macos::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub(crate) use windows::*;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub(crate) use linux::*;

#[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
mod other;
#[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
pub(crate) use other::*;

#[cfg(not(target_os = "macos"))]
mod no_workgroups;
#[cfg(not(target_os = "macos"))]
pub(crate) use no_workgroups::*;
