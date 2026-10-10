//! RealtimeSanitizer (LLVM's RTSan) around real-time sections.
//!
//! RTSan replaces `malloc`, `pthread_mutex_lock`, `sleep`, file and socket
//! calls, and a few hundred more with versions that check whether the
//! calling thread is inside a real-time section, and stop the program with a
//! report and a stack trace if it is. That catches what the allocation trap
//! can't: locks, blocking system calls, and allocation by C code.
//!
//! It costs nothing unless a build asks for it. Three things must line up:
//!
//! 1. The `rtsan` feature of this crate (or of `bl-audio-io`, which passes
//!    it on).
//! 2. `RTSAN_ENABLE` set while building, which makes `rtsan-standalone-sys`
//!    link the runtime.
//! 3. `RTSAN_LIB_PATH` naming a runtime already on the machine:
//!    `libclang_rt.rtsan_osx_dynamic.dylib` from Homebrew's `llvm` on macOS,
//!    or `libclang_rt.rtsan*.a` from LLVM 20 or later on Linux. Without it,
//!    the build script would fetch and build LLVM's source, which Beatlayer's
//!    rules forbid.
//!
//! `cargo xtask rtsan` finds the runtime, sets all three, and runs the tests
//! written for RTSan, in their own target directory so the normal build isn't
//! disturbed.

/// Whether this build runs real-time sections under RealtimeSanitizer.
pub const ACTIVE: bool = cfg!(feature = "rtsan")
    && option_env!("RTSAN_ENABLE").is_some()
    && cfg!(any(
        target_os = "macos",
        all(
            target_os = "linux",
            any(target_arch = "x86_64", target_arch = "aarch64")
        )
    ));

/// Marks the calling thread as inside a real-time section until it drops.
#[derive(Debug)]
pub(crate) struct Scope {
    _not_send: std::marker::PhantomData<*const ()>,
}

impl Scope {
    #[inline]
    pub(crate) fn enter() -> Self {
        #[cfg(feature = "rtsan")]
        rtsan_standalone::realtime_enter();
        Self {
            _not_send: std::marker::PhantomData,
        }
    }
}

impl Drop for Scope {
    #[inline]
    fn drop(&mut self) {
        #[cfg(feature = "rtsan")]
        rtsan_standalone::realtime_exit();
    }
}

/// Initialises the sanitizer's runtime, if it's linked. Set-up calls this so
/// the runtime's own first-use work never happens inside a real-time section.
pub fn ensure_initialized() {
    #[cfg(feature = "rtsan")]
    rtsan_standalone::ensure_initialized();
}

/// Runs `f` with RTSan's reports switched off, for a call that RTSan can't
/// see is safe. Each use needs a comment saying why it's safe.
pub fn unchecked<R>(f: impl FnOnce() -> R) -> R {
    #[cfg(feature = "rtsan")]
    let _off = rtsan_standalone::ScopedDisabler::default();
    f()
}

/// Tells RTSan that the calling function blocks, so calling it from a
/// real-time section is reported even if nothing it calls is intercepted:
/// for example a function that spins on a lock built from atomics.
pub fn notify_blocking(function: &'static std::ffi::CStr) {
    #[cfg(feature = "rtsan")]
    rtsan_standalone::notify_blocking_call(function);
    #[cfg(not(feature = "rtsan"))]
    let _ = function;
}
