//! Windows: the Multimedia Class Scheduler Service (MMCSS), task "Pro Audio".
//!
//! MMCSS schedules a registered thread in the real-time priority range for
//! its task's share of each period. "Pro Audio" is the task meant for audio
//! engines; the `audio_thread_priority` crate registers "Audio" instead,
//! which is one reason Beatlayer writes this itself (see the architecture).

use std::cell::Cell;

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Threading::{
    AVRT_PRIORITY_CRITICAL, AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW,
    AvSetMmThreadPriority, GetCurrentThread, GetThreadPriority,
};
use windows::core::w;

use crate::thread::{Method, Priority, Promotion, Refusal};

const TASK: &str = "Pro Audio";

thread_local! {
    /// The task index of this thread's MMCSS registration, if it has one.
    static TASK_INDEX: Cell<Option<u32>> = const { Cell::new(None) };
}

/// This thread's MMCSS registration, reverted when the thread exits.
#[derive(Debug, Default)]
pub(crate) struct HeldPriority {
    handle: Option<isize>,
}

impl Drop for HeldPriority {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            // SAFETY: the handle came from AvSetMmThreadCharacteristicsW on
            // this thread (thread-local state drops on its own thread).
            let _ = unsafe { AvRevertMmThreadCharacteristics(HANDLE(handle as *mut _)) };
            let _ = TASK_INDEX.try_with(|index| index.set(None));
        }
    }
}

fn thread_priority() -> i32 {
    // SAFETY: the pseudo-handle for the current thread is always valid.
    unsafe { GetThreadPriority(GetCurrentThread()) }
}

pub(crate) fn current_priority() -> Priority {
    match TASK_INDEX.try_with(Cell::get).ok().flatten() {
        Some(task_index) => Priority::Mmcss {
            task: TASK,
            task_index,
            level: thread_priority(),
        },
        None => Priority::Normal {
            level: thread_priority(),
        },
    }
}

pub(crate) fn promote(_period: std::time::Duration, held: &mut HeldPriority) -> Promotion {
    let mut task_index = 0u32;
    // SAFETY: the task name is a valid wide string and `task_index` is
    // writable.
    let handle = match unsafe { AvSetMmThreadCharacteristicsW(w!("Pro Audio"), &mut task_index) } {
        Ok(handle) => handle,
        Err(error) => return Promotion::Refused(Method::Mmcss, Refusal::Os(error.code().0)),
    };
    // SAFETY: `handle` was just returned for this thread.
    if let Err(error) = unsafe { AvSetMmThreadPriority(handle, AVRT_PRIORITY_CRITICAL) } {
        // SAFETY: as above.
        let _ = unsafe { AvRevertMmThreadCharacteristics(handle) };
        return Promotion::Refused(Method::Mmcss, Refusal::Os(error.code().0));
    }
    held.handle = Some(handle.0 as isize);
    let _ = TASK_INDEX.try_with(|index| index.set(Some(task_index)));
    Promotion::Raised(Method::Mmcss)
}
