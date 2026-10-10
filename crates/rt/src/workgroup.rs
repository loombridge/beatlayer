//! macOS audio workgroups (`os_workgroup`), through Beatlayer's own bindings.
//!
//! A workgroup tells the macOS scheduler which real-time threads work to the
//! same deadline, so it can keep them on performance cores and wake them
//! together. Core Audio makes one per audio device
//! (`kAudioDevicePropertyIOThreadOSWorkgroup`) and puts the device's I/O
//! thread in it. Any other thread that works to that device's deadline, such
//! as Beatlayer's real-time worker pool, joins it with [`Workgroup::join`].
//! Threads that keep their own time, such as the null device's, use a
//! [`WorkInterval`] instead.
//!
//! As of October 2026 no published crate binds these calls, so they're
//! declared in `platform/macos.rs`. On other platforms every constructor
//! returns an error and no [`Workgroup`] can exist.

// Elsewhere the raw types are uninhabited, so the compiler knows code after
// a platform call can't run; it's still written for macOS.
#![cfg_attr(not(target_os = "macos"), allow(unreachable_code, unused_variables))]

use std::cell::Cell;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use crate::platform;

/// Whether a thread is in its workgroup, after set-up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WorkgroupJoin {
    /// There was no workgroup to join: the platform has none, or the device
    /// doesn't offer one.
    #[default]
    None,
    /// The thread joined it.
    Joined,
    /// The thread already belongs to a workgroup this one doesn't nest with
    /// (`EALREADY`). Core Audio's I/O thread is a member of its device's
    /// workgroup from the start, so this is the expected answer there.
    AlreadyMember,
    /// The join failed with this `errno`; `EINVAL` means the workgroup was
    /// cancelled, for example because its device went away.
    Failed(i32),
}

impl fmt::Display for WorkgroupJoin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => f.write_str("none"),
            Self::Joined => f.write_str("joined"),
            Self::AlreadyMember => f.write_str("already a member"),
            Self::Failed(code) => write!(f, "join failed with errno {code}"),
        }
    }
}

/// Why a workgroup couldn't be found or made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkgroupError {
    message: String,
}

impl WorkgroupError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for WorkgroupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for WorkgroupError {}

/// An audio workgroup. Cloning is cheap: clones share one retained OS object,
/// which is released when the last clone drops.
#[derive(Clone)]
pub struct Workgroup {
    raw: platform::RawWorkgroup,
    label: Arc<str>,
}

impl fmt::Debug for Workgroup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Workgroup")
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}

impl Workgroup {
    /// The workgroup of the I/O thread of the Core Audio device with this UID
    /// (`kAudioDevicePropertyDeviceUID`, which is what `cpal` calls a Core
    /// Audio device's ID).
    pub fn of_core_audio_device(uid: &str) -> Result<Self, WorkgroupError> {
        let (raw, name) = platform::device_workgroup(Some(uid))?;
        Ok(Self {
            raw,
            label: format!("the I/O workgroup of {name}").into(),
        })
    }

    /// The workgroup of the system default output device's I/O thread.
    pub fn of_default_output_device() -> Result<Self, WorkgroupError> {
        let (raw, name) = platform::device_workgroup(None)?;
        Ok(Self {
            raw,
            label: format!("the I/O workgroup of {name}").into(),
        })
    }

    /// Where this workgroup comes from, for a person to read: for example
    /// "the I/O workgroup of MacBook Pro Speakers".
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The operating system's own description of the workgroup object, which
    /// includes its name.
    pub fn description(&self) -> String {
        platform::workgroup_description(&self.raw)
    }

    /// How many threads the system suggests running in parallel in this
    /// workgroup, which is how big a real-time worker pool should be.
    pub fn max_parallel_threads(&self) -> Option<u32> {
        platform::workgroup_max_parallel_threads(&self.raw)
    }

    /// Joins the calling thread to this workgroup until the token drops. The
    /// thread should already have real-time priority. Real-time safe apart
    /// from one small allocation for the token, so call it during set-up.
    pub fn join(&self) -> Result<JoinToken, WorkgroupJoin> {
        let raw = platform::workgroup_join(&self.raw).map_err(|code| match code {
            libc_errno::EALREADY => WorkgroupJoin::AlreadyMember,
            code => WorkgroupJoin::Failed(code),
        })?;
        let previous = JOINED.replace(platform::workgroup_addr(&self.raw));
        Ok(JoinToken {
            workgroup: self.clone(),
            raw: Some(raw),
            previous,
            _not_send: std::marker::PhantomData,
        })
    }

    /// Whether the calling thread joined this workgroup through
    /// [`join`](Self::join) and hasn't left. Real-time safe.
    pub fn is_joined_here(&self) -> bool {
        JOINED.try_with(Cell::get).unwrap_or(0) == platform::workgroup_addr(&self.raw)
    }
}

thread_local! {
    /// The address of the workgroup this thread most recently joined, or 0.
    /// macOS stops a process that starts a work interval from a thread
    /// outside it, so [`WorkInterval::start`] checks this first.
    static JOINED: Cell<usize> = const { Cell::new(0) };
}

/// `errno` values, which are the same on every platform that has workgroups.
mod libc_errno {
    pub(super) const EPERM: i32 = 1;
    pub(super) const EALREADY: i32 = 37;
}

/// Membership of a workgroup for the thread that joined. Dropping it leaves
/// the workgroup, which must happen on the same thread, so it isn't `Send`.
pub struct JoinToken {
    workgroup: Workgroup,
    raw: Option<platform::RawJoinToken>,
    /// What `JOINED` held before this join.
    previous: usize,
    _not_send: std::marker::PhantomData<*const ()>,
}

impl fmt::Debug for JoinToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JoinToken")
            .field("workgroup", &self.workgroup)
            .finish_non_exhaustive()
    }
}

impl Drop for JoinToken {
    fn drop(&mut self) {
        if let Some(raw) = self.raw.take() {
            platform::workgroup_leave(&self.workgroup.raw, raw);
            let _ = JOINED.try_with(|joined| joined.set(self.previous));
        }
    }
}

/// A workgroup with its own deadlines, for real-time threads that keep their
/// own time rather than a device's (Apple's `AudioWorkIntervalCreate`).
///
/// The thread that drives the work joins [`workgroup`](Self::workgroup), then
/// brackets each cycle with [`start`](Self::start) and
/// [`finish`](Self::finish).
#[derive(Debug, Clone)]
pub struct WorkInterval {
    workgroup: Workgroup,
}

impl WorkInterval {
    /// A new work interval called `name`.
    pub fn new(name: &str) -> Result<Self, WorkgroupError> {
        let raw = platform::work_interval_create(name)?;
        Ok(Self {
            workgroup: Workgroup {
                raw,
                label: format!("the work interval \"{name}\"").into(),
            },
        })
    }

    /// The interval's workgroup, which its threads join.
    pub fn workgroup(&self) -> &Workgroup {
        &self.workgroup
    }

    /// Tells the system a cycle starts now and must finish within `length`.
    /// Real-time safe.
    ///
    /// The calling thread must have joined the workgroup. If it hasn't, this
    /// returns `EPERM` without asking the system, because macOS stops the
    /// process rather than return the error its header documents.
    pub fn start(&self, length: Duration) -> Result<(), i32> {
        if !self.workgroup.is_joined_here() {
            return Err(libc_errno::EPERM);
        }
        platform::work_interval_start(&self.workgroup.raw, length)
    }

    /// Tells the system the current cycle is done. Real-time safe. Like
    /// [`start`](Self::start), it needs the calling thread to be a member.
    pub fn finish(&self) -> Result<(), i32> {
        if !self.workgroup.is_joined_here() {
            return Err(libc_errno::EPERM);
        }
        platform::work_interval_finish(&self.workgroup.raw)
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use crate::thread::{Promotion, RtThreadConfig, prepare_current_thread};

    #[test]
    fn a_work_interval_can_be_joined_started_and_left() {
        let interval = WorkInterval::new("bl-rt test").unwrap();
        assert!(interval.workgroup().label().contains("bl-rt test"));
        let description = interval.workgroup().description();
        assert!(!description.is_empty());
        let max = interval.workgroup().max_parallel_threads();
        assert!(max.is_some_and(|n| n >= 1), "{max:?}");

        let worker = interval.clone();
        std::thread::spawn(move || {
            let report = prepare_current_thread(&RtThreadConfig {
                period: Duration::from_millis(5),
                promote: true,
                workgroup: Some(worker.workgroup().clone()),
            });
            assert!(matches!(report.promotion, Promotion::Raised(_)));
            assert_eq!(report.workgroup, WorkgroupJoin::Joined);
            for _ in 0..3 {
                worker.start(Duration::from_millis(5)).unwrap();
                worker.finish().unwrap();
            }
        })
        .join()
        .unwrap();
    }

    #[test]
    fn a_thread_cannot_be_in_two_workgroups_that_do_not_nest() {
        let first = WorkInterval::new("first").unwrap();
        let second = WorkInterval::new("second").unwrap();
        std::thread::spawn(move || {
            let token = first.workgroup().join().unwrap();
            let again = second.workgroup().join().map(drop);
            assert_eq!(again, Err(WorkgroupJoin::AlreadyMember));
            drop(token);
            // Having left the first, the thread can join the second.
            second.workgroup().join().unwrap();
        })
        .join()
        .unwrap();
    }

    #[test]
    fn starting_an_interval_needs_membership() {
        let interval = WorkInterval::new("not joined").unwrap();
        let five_ms = Duration::from_millis(5);
        assert_eq!(interval.start(five_ms), Err(libc_errno::EPERM));
        assert_eq!(interval.finish(), Err(libc_errno::EPERM));
        std::thread::spawn(move || {
            let token = interval.workgroup().join().unwrap();
            assert!(interval.workgroup().is_joined_here());
            interval.start(five_ms).unwrap();
            interval.finish().unwrap();
            drop(token);
            assert!(!interval.workgroup().is_joined_here());
            assert_eq!(interval.start(five_ms), Err(libc_errno::EPERM));
        })
        .join()
        .unwrap();
    }

    /// Needs an output device; Mac CI runners may have none, so the test
    /// says so and passes rather than failing.
    #[test]
    fn the_default_output_device_has_a_workgroup() {
        match Workgroup::of_default_output_device() {
            Ok(workgroup) => {
                assert!(workgroup.label().starts_with("the I/O workgroup of "));
                let joined = std::thread::spawn(move || workgroup.join().map(drop))
                    .join()
                    .unwrap();
                assert_eq!(
                    joined,
                    Ok(()),
                    "a fresh thread can join a device's workgroup"
                );
            }
            Err(error) => eprintln!("skipped: {error}"),
        }
    }

    #[test]
    fn an_unknown_device_uid_is_an_error() {
        let error = Workgroup::of_core_audio_device("no-such-device-uid").unwrap_err();
        assert!(error.to_string().contains("no-such-device-uid"), "{error}");
    }
}
