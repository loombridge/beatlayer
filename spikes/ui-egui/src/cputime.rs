//! CPU time used by the calling thread, for per-frame CPU cost.

use std::time::Duration;

/// CPU time (user and system) the calling thread has used so far.
#[cfg(unix)]
pub fn thread_cpu_time() -> Duration {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` is a valid, writable timespec, and CLOCK_THREAD_CPUTIME_ID
    // is supported on macOS 10.12 and later and on Linux.
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
    if rc != 0 {
        return Duration::ZERO;
    }
    Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32)
}

/// CPU time (user and kernel) the calling thread has used so far.
#[cfg(windows)]
pub fn thread_cpu_time() -> Duration {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::{GetCurrentThread, GetThreadTimes};

    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut creation, mut exit, mut kernel, mut user) = (zero, zero, zero, zero);
    // SAFETY: the pseudo-handle from GetCurrentThread is always valid, and
    // every out-pointer refers to a live FILETIME.
    let ok = unsafe {
        GetThreadTimes(
            GetCurrentThread(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    };
    if ok == 0 {
        return Duration::ZERO;
    }
    let ticks = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
    // FILETIME counts 100 ns intervals.
    Duration::from_nanos((ticks(kernel) + ticks(user)) * 100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_work_uses_cpu_time() {
        let before = thread_cpu_time();
        let mut x = 0u64;
        for i in 0..20_000_000u64 {
            x = x.wrapping_mul(31).wrapping_add(i);
        }
        std::hint::black_box(x);
        assert!(thread_cpu_time() > before);
    }
}
