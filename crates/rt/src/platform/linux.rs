//! Linux: `SCHED_FIFO` directly where the process may set it, otherwise
//! through rtkit.
//!
//! The direct call works when `RLIMIT_RTPRIO` allows it (a member of an
//! `audio` group with rtprio limits, as audio-focused distributions set up) or
//! the process has `CAP_SYS_NICE`. It's one system call, so it comes first.
//! Desktop distributions without those limits run rtkit, a system service
//! that grants real-time scheduling (`SCHED_RR`, at most priority 20 by
//! default) to threads that ask over D-Bus.

use std::time::Duration;

use crate::rtkit;
use crate::thread::{Method, Priority, Promotion};

/// The real-time priority Beatlayer asks for: in the middle of the range and
/// below PipeWire's own audio threads (88 by default), so an engine thread
/// never preempts the sound server it feeds. rtkit lowers it to its own cap.
const WANTED: i32 = 70;

/// Nothing to undo on Linux: the policy goes with the thread.
#[derive(Debug, Default)]
pub(crate) struct HeldPriority;

pub(crate) fn current_priority() -> Priority {
    let mut policy = 0;
    // SAFETY: `sched_param` is plain data, so zeroed is a valid value.
    let mut param: libc::sched_param = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers are to live locals.
    if unsafe { libc::pthread_getschedparam(libc::pthread_self(), &mut policy, &mut param) } != 0 {
        return Priority::Unknown;
    }
    match policy & !libc::SCHED_RESET_ON_FORK {
        libc::SCHED_FIFO => Priority::Fifo {
            level: param.sched_priority,
        },
        libc::SCHED_RR => Priority::RoundRobin {
            level: param.sched_priority,
        },
        _ => {
            // SAFETY: `gettid` always succeeds.
            let tid = unsafe { libc::gettid() };
            // SAFETY: reads the nice value of this thread. -1 is also a valid
            // nice value, so an error can't be told apart; it doesn't matter
            // for a report.
            let nice = unsafe { libc::getpriority(libc::PRIO_PROCESS, tid as libc::id_t) };
            Priority::Normal { level: nice }
        }
    }
}

/// The highest real-time priority this process may set by itself, if any.
fn rtprio_limit() -> Option<i32> {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `limit` is writable.
    if unsafe { libc::getrlimit(libc::RLIMIT_RTPRIO, &mut limit) } != 0 {
        return None;
    }
    if limit.rlim_cur == libc::RLIM_INFINITY {
        Some(i32::MAX)
    } else {
        i32::try_from(limit.rlim_cur).ok()
    }
}

fn set_fifo(level: i32) -> Result<(), i32> {
    let param = libc::sched_param {
        sched_priority: level,
    };
    // SAFETY: `param` is a valid `sched_param`. SCHED_RESET_ON_FORK keeps
    // child processes from inheriting real-time scheduling.
    let status = unsafe {
        libc::pthread_setschedparam(
            libc::pthread_self(),
            libc::SCHED_FIFO | libc::SCHED_RESET_ON_FORK,
            &param,
        )
    };
    if status == 0 { Ok(()) } else { Err(status) }
}

pub(crate) fn promote(_period: Duration, _held: &mut HeldPriority) -> Promotion {
    // SAFETY: no preconditions.
    let max = unsafe { libc::sched_get_priority_max(libc::SCHED_FIFO) };
    let level = match rtprio_limit() {
        // A limit of zero still lets a process with CAP_SYS_NICE through.
        Some(limit) if limit > 0 => WANTED.min(limit),
        _ => WANTED,
    }
    .min(max)
    .max(1);
    if set_fifo(level).is_ok() {
        return Promotion::Raised(Method::SchedFifo);
    }
    match rtkit::make_current_thread_realtime(WANTED) {
        Ok(_) => Promotion::Raised(Method::Rtkit),
        Err(refusal) => Promotion::Refused(Method::Rtkit, refusal),
    }
}
