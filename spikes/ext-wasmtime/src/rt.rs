//! The real-time section: with `--cfg ext_rtsan` (build.rs sets it when
//! RTSAN_LIB_PATH names a runtime), entering it makes RealtimeSanitizer stop the
//! process at any call that allocates, locks or blocks. Without it, entering
//! does nothing and [`ACTIVE`] is false.

pub const ACTIVE: bool = cfg!(ext_rtsan);

#[cfg(ext_rtsan)]
unsafe extern "C" {
    fn __rtsan_ensure_initialized();
    fn __rtsan_realtime_enter();
    fn __rtsan_realtime_exit();
    fn __rtsan_disable();
    fn __rtsan_enable();
}

/// A real-time section, left when dropped.
pub struct Realtime(());

impl Realtime {
    pub fn enter() -> Self {
        #[cfg(ext_rtsan)]
        // SAFETY: the runtime's entry points take no arguments and may be
        // called from any thread.
        unsafe {
            __rtsan_ensure_initialized();
            __rtsan_realtime_enter();
        }
        Realtime(())
    }
}

impl Drop for Realtime {
    fn drop(&mut self) {
        #[cfg(ext_rtsan)]
        // SAFETY: as in `enter`; this pairs with its call.
        unsafe {
            __rtsan_realtime_exit();
        }
    }
}

/// Runs `f` with RealtimeSanitizer's checks off on this thread, for work a
/// real-time thread is allowed to leave to the system, such as the handler
/// that runs when a guest traps.
pub fn unchecked<R>(f: impl FnOnce() -> R) -> R {
    #[cfg(ext_rtsan)]
    // SAFETY: as in `Realtime::enter`.
    unsafe {
        __rtsan_disable();
    }
    let result = f();
    #[cfg(ext_rtsan)]
    // SAFETY: pairs with the call above.
    unsafe {
        __rtsan_enable();
    }
    result
}
