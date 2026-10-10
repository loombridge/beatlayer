//! Real-time priority for audio threads, and the set-up every real-time
//! thread runs once.
//!
//! | Platform | Real-time class | How Beatlayer asks for it |
//! |---|---|---|
//! | macOS | Mach time-constraint policy | `thread_policy_set`, then joins the audio workgroup |
//! | Windows | MMCSS "Pro Audio" task | `AvSetMmThreadCharacteristicsW` |
//! | Linux | `SCHED_FIFO` (or rtkit's `SCHED_RR`) | `pthread_setschedparam`, then rtkit over D-Bus |
//!
//! A thread an audio API already runs in real time (Core Audio's I/O thread,
//! JACK's process thread) is left as it is. On macOS that thread is also
//! already a member of its device's workgroup, which Apple documents for
//! `kAudioDevicePropertyIOThreadOSWorkgroup`.

use std::cell::{Cell, RefCell};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::Duration;

use crate::platform;
use crate::workgroup::{JoinToken, Workgroup, WorkgroupJoin};

/// How the operating system schedules a thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Priority {
    /// This platform doesn't say.
    #[default]
    Unknown,
    /// Ordinary time-sharing: the thread competes with every other thread.
    /// `level` is the POSIX priority on macOS, the nice value on Linux, and
    /// `GetThreadPriority` on Windows.
    Normal {
        /// The platform's priority number.
        level: i32,
    },
    /// macOS's real-time class, the Mach time-constraint policy: the thread
    /// needs `computation` of CPU time in every `period`, finished within
    /// `constraint` of the period's start.
    TimeConstraint {
        /// How often the work repeats.
        period: Duration,
        /// How much CPU time each period needs.
        computation: Duration,
        /// The deadline, from the start of each period.
        constraint: Duration,
    },
    /// A Windows Multimedia Class Scheduler Service task, which the service
    /// schedules in the real-time range.
    Mmcss {
        /// The task's name, such as "Pro Audio".
        task: &'static str,
        /// The task index MMCSS assigned.
        task_index: u32,
        /// `GetThreadPriority` after registering.
        level: i32,
    },
    /// POSIX `SCHED_FIFO`.
    Fifo {
        /// The real-time priority, 1 to 99 on Linux.
        level: i32,
    },
    /// POSIX `SCHED_RR`, which rtkit grants.
    RoundRobin {
        /// The real-time priority.
        level: i32,
    },
}

impl Priority {
    /// Whether this is a real-time class.
    pub fn is_realtime(&self) -> bool {
        !matches!(self, Self::Unknown | Self::Normal { .. })
    }
}

impl fmt::Display for Priority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ms = |d: &Duration| d.as_secs_f64() * 1000.0;
        match self {
            Self::Unknown => f.write_str("unknown"),
            Self::Normal { level } => write!(f, "normal (priority {level})"),
            Self::TimeConstraint {
                period,
                computation,
                constraint,
            } => write!(
                f,
                "real-time, Mach time-constraint policy (period {:.3} ms, computation {:.3} ms, constraint {:.3} ms)",
                ms(period),
                ms(computation),
                ms(constraint)
            ),
            Self::Mmcss {
                task,
                task_index,
                level,
            } => write!(
                f,
                "real-time, MMCSS task \"{task}\" (index {task_index}, thread priority {level})"
            ),
            Self::Fifo { level } => write!(f, "real-time, SCHED_FIFO priority {level}"),
            Self::RoundRobin { level } => write!(f, "real-time, SCHED_RR priority {level}"),
        }
    }
}

/// A way of raising a thread to real-time priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// macOS `thread_policy_set` with the time-constraint policy.
    TimeConstraint,
    /// Windows MMCSS, task "Pro Audio".
    Mmcss,
    /// POSIX `pthread_setschedparam` with `SCHED_FIFO`.
    SchedFifo,
    /// rtkit's `MakeThreadRealtime` over the D-Bus system bus (Linux).
    Rtkit,
}

impl fmt::Display for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::TimeConstraint => "the Mach time-constraint policy",
            Self::Mmcss => "MMCSS",
            Self::SchedFifo => "SCHED_FIFO",
            Self::Rtkit => "rtkit",
        })
    }
}

/// Why a request for real-time priority failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The system call failed with this code: an `errno`, a Mach
    /// `kern_return_t`, or a Windows error.
    Os(i32),
    /// No D-Bus system bus to reach rtkit on.
    NoBus,
    /// rtkit isn't running.
    NoRtkit,
    /// rtkit said no. It refuses processes that ask too often, threads that
    /// aren't the caller's, and priorities above its limit.
    RtkitDenied,
    /// The conversation with D-Bus or rtkit went wrong.
    Protocol,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Os(code) => write!(f, "error {code}"),
            Self::NoBus => f.write_str("no D-Bus system bus"),
            Self::NoRtkit => f.write_str("rtkit isn't running"),
            Self::RtkitDenied => f.write_str("rtkit refused"),
            Self::Protocol => f.write_str("a D-Bus protocol error"),
        }
    }
}

/// What set-up did about the thread's priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Promotion {
    /// The thread wasn't to be raised, for example because it runs a test
    /// device as fast as it can.
    #[default]
    NotRequested,
    /// The thread was already real-time: the audio API had set it up.
    AlreadyRealtime,
    /// Beatlayer raised it.
    Raised(Method),
    /// Beatlayer asked and was refused; the last method tried is named.
    Refused(Method, Refusal),
    /// This platform has no way to ask.
    Unsupported,
}

impl fmt::Display for Promotion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRequested => f.write_str("not requested"),
            Self::AlreadyRealtime => f.write_str("already real-time when Beatlayer got it"),
            Self::Raised(method) => write!(f, "raised by Beatlayer through {method}"),
            Self::Refused(method, why) => write!(f, "refused: {method} failed with {why}"),
            Self::Unsupported => f.write_str("not supported on this platform"),
        }
    }
}

/// What a real-time thread's set-up asks for.
#[derive(Debug, Clone, Default)]
pub struct RtThreadConfig {
    /// How often the thread's work repeats: one audio block. macOS uses it for
    /// the time-constraint policy.
    pub period: Duration,
    /// Whether to raise the thread to real-time priority if it isn't already.
    pub promote: bool,
    /// The workgroup to join, on macOS: the output device's, so the system
    /// schedules this thread with the device's I/O thread.
    pub workgroup: Option<Workgroup>,
}

/// The state of a real-time thread after [`prepare_current_thread`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ThreadReport {
    /// How the thread is scheduled now.
    pub priority: Priority,
    /// What set-up did about it.
    pub promotion: Promotion,
    /// Whether the thread is in the workgroup (macOS).
    pub workgroup: WorkgroupJoin,
    /// Whether denormals are flushed to zero inside real-time sections.
    pub denormals_flushed: bool,
    /// Whether the allocation trap is the global allocator, so real-time
    /// sections count heap use.
    pub trap_installed: bool,
    /// Whether RealtimeSanitizer watches real-time sections.
    pub sanitizer_active: bool,
}

/// Thread-local state that keeps a thread's real-time set-up alive and undoes
/// it when the thread exits: the MMCSS registration on Windows and the
/// workgroup membership on macOS.
#[derive(Default)]
struct Held {
    joined: Option<JoinToken>,
    platform: platform::HeldPriority,
}

thread_local! {
    static SERIAL: Cell<u64> = const { Cell::new(0) };
    static HELD: RefCell<Option<Held>> = const { RefCell::new(None) };
}

static NEXT_SERIAL: AtomicU64 = AtomicU64::new(1);

/// A number for the calling thread that no other thread in this process gets,
/// even after this one exits. Real-time safe after its first call on a thread.
///
/// Thread IDs from the operating system can be reused, and Core Audio moves
/// its I/O thread when the default device changes, so per-thread set-up keys
/// on this instead.
pub fn serial() -> u64 {
    SERIAL
        .try_with(|serial| {
            if serial.get() == 0 {
                serial.set(NEXT_SERIAL.fetch_add(1, Relaxed));
            }
            serial.get()
        })
        .unwrap_or(0)
}

/// How the calling thread is scheduled now.
pub fn current_priority() -> Priority {
    platform::current_priority()
}

/// Sets up the calling thread for real-time work: raises its priority if
/// asked and needed, joins the workgroup, and reports the result.
///
/// Call it once on each thread before its first real-time section; calling
/// it again on the same thread replaces the earlier set-up. It may allocate,
/// make system calls, and (on Linux) talk to rtkit over D-Bus, so it never
/// runs inside a real-time section. What it acquires is released when the
/// thread exits.
pub fn prepare_current_thread(config: &RtThreadConfig) -> ThreadReport {
    // Touch every thread-local a real-time section uses, so none of them is
    // first allocated inside one (macOS allocates thread-local storage lazily).
    serial();
    crate::log::register_current_thread();
    let denormals_flushed = crate::realtime(crate::denormals::flushing);

    HELD.with(|held| {
        let mut held = held.borrow_mut();
        let held = held.get_or_insert_with(Held::default);
        // Leave a previous workgroup before joining another: workgroups must
        // be left in the reverse order they were joined.
        held.joined = None;

        let before = platform::current_priority();
        let promotion = if before.is_realtime() {
            Promotion::AlreadyRealtime
        } else if config.promote {
            platform::promote(config.period, &mut held.platform)
        } else {
            Promotion::NotRequested
        };

        let workgroup = match &config.workgroup {
            None => WorkgroupJoin::None,
            Some(workgroup) => match workgroup.join() {
                Ok(token) => {
                    held.joined = Some(token);
                    WorkgroupJoin::Joined
                }
                Err(join) => join,
            },
        };

        ThreadReport {
            priority: platform::current_priority(),
            promotion,
            workgroup,
            denormals_flushed,
            trap_installed: crate::trap::is_installed(),
            sanitizer_active: crate::sanitize::ACTIVE,
        }
    })
}

/// Undoes what [`prepare_current_thread`] acquired on the calling thread:
/// leaves the workgroup and ends the MMCSS registration. The priority itself
/// stays. A thread Beatlayer owns calls this before it ends, because the
/// order in which a thread's clean-up runs at exit isn't promised.
pub fn release_current_thread() {
    let held = HELD
        .try_with(|held| held.borrow_mut().take())
        .ok()
        .flatten();
    drop(held);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serials_are_unique_per_thread_and_stable_within_one() {
        let here = serial();
        assert_ne!(here, 0);
        assert_eq!(serial(), here);
        let there = std::thread::spawn(serial).join().unwrap();
        assert_ne!(there, here);
        let again = std::thread::spawn(serial).join().unwrap();
        assert_ne!(again, there, "an exited thread's serial isn't reused");
    }

    #[test]
    fn a_test_thread_starts_with_normal_priority() {
        let priority = current_priority();
        assert!(!priority.is_realtime(), "{priority}");
    }

    #[test]
    fn set_up_without_promotion_changes_nothing() {
        let report = std::thread::spawn(|| {
            prepare_current_thread(&RtThreadConfig {
                period: Duration::from_millis(5),
                promote: false,
                workgroup: None,
            })
        })
        .join()
        .unwrap();
        assert_eq!(report.promotion, Promotion::NotRequested);
        assert!(!report.priority.is_realtime());
        assert_eq!(report.workgroup, WorkgroupJoin::None);
        assert_eq!(report.denormals_flushed, crate::denormals::SUPPORTED);
        assert!(report.trap_installed);
    }

    /// Raising a thread's priority works without special rights on macOS and
    /// Windows. Linux needs rtprio limits or rtkit, which CI checks.
    #[cfg(any(target_os = "macos", windows))]
    #[test]
    fn set_up_raises_a_thread_to_real_time() {
        let (report, after_exit) = std::thread::spawn(|| {
            let report = prepare_current_thread(&RtThreadConfig {
                period: Duration::from_micros(2_667),
                promote: true,
                workgroup: None,
            });
            (report, current_priority())
        })
        .join()
        .unwrap();
        assert!(
            matches!(report.promotion, Promotion::Raised(_)),
            "{}",
            report.promotion
        );
        assert!(report.priority.is_realtime(), "{}", report.priority);
        assert!(after_exit.is_realtime());
        #[cfg(target_os = "macos")]
        assert!(
            matches!(report.priority, Priority::TimeConstraint { period, .. }
                if period.abs_diff(Duration::from_micros(2_667)) < Duration::from_micros(1)),
            "{}",
            report.priority
        );
        #[cfg(windows)]
        assert!(matches!(
            report.priority,
            Priority::Mmcss {
                task: "Pro Audio",
                ..
            }
        ));
    }

    #[cfg(any(target_os = "macos", windows))]
    #[test]
    fn a_thread_that_is_already_real_time_is_left_alone() {
        let second = std::thread::spawn(|| {
            let config = RtThreadConfig {
                period: Duration::from_millis(10),
                promote: true,
                workgroup: None,
            };
            prepare_current_thread(&config);
            prepare_current_thread(&config)
        })
        .join()
        .unwrap();
        assert_eq!(second.promotion, Promotion::AlreadyRealtime);
        assert!(second.priority.is_realtime());
    }

    #[test]
    fn priorities_describe_themselves() {
        let tc = Priority::TimeConstraint {
            period: Duration::from_micros(2_667),
            computation: Duration::from_micros(1_000),
            constraint: Duration::from_micros(2_667),
        };
        assert_eq!(
            tc.to_string(),
            "real-time, Mach time-constraint policy (period 2.667 ms, computation 1.000 ms, constraint 2.667 ms)"
        );
        assert_eq!(
            Priority::Normal { level: 31 }.to_string(),
            "normal (priority 31)"
        );
        assert_eq!(
            Promotion::Refused(Method::Rtkit, Refusal::NoRtkit).to_string(),
            "refused: rtkit failed with rtkit isn't running"
        );
    }
}
