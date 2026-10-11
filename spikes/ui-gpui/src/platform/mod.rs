//! Platform code the toolkit doesn't cover. Everything outside this module
//! is platform-independent.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(not(target_os = "macos"))]
mod other;
#[cfg(not(target_os = "macos"))]
pub use other::*;

/// CPU time used so far by the calling thread and by the whole process, in
/// seconds.
#[cfg(unix)]
pub fn cpu_times() -> (f64, f64) {
    fn secs(ts: libc::timespec) -> f64 {
        ts.tv_sec as f64 + ts.tv_nsec as f64 * 1e-9
    }
    let mut thread = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    let mut process = thread;
    // SAFETY: both pointers are to valid, writable timespecs.
    unsafe {
        libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut thread);
        libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut process);
    }
    (secs(thread), secs(process))
}

/// Windows would use `GetThreadTimes` and `GetProcessTimes`; the Windows run
/// is pending (no test machines yet), so the bench reports CPU time as unavailable there.
#[cfg(not(unix))]
pub fn cpu_times() -> (f64, f64) {
    (f64::NAN, f64::NAN)
}
