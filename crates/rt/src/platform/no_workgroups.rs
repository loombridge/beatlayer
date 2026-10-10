//! Every platform but macOS: no audio workgroups. No value of the raw types
//! exists, so the public `Workgroup` can't be constructed and its methods
//! are unreachable.

use crate::workgroup::WorkgroupError;

/// An uninhabited stand-in for `os_workgroup_t`.
#[derive(Debug, Clone)]
pub(crate) enum RawWorkgroup {}

/// Never exists, like [`RawWorkgroup`].
pub(crate) enum RawJoinToken {}

const NO_WORKGROUPS: &str = "audio workgroups exist only on macOS";

pub(crate) fn device_workgroup(
    _uid: Option<&str>,
) -> Result<(RawWorkgroup, String), WorkgroupError> {
    Err(WorkgroupError::new(NO_WORKGROUPS))
}

pub(crate) fn work_interval_create(_name: &str) -> Result<RawWorkgroup, WorkgroupError> {
    Err(WorkgroupError::new(NO_WORKGROUPS))
}

pub(crate) fn workgroup_description(workgroup: &RawWorkgroup) -> String {
    match *workgroup {}
}

pub(crate) fn workgroup_addr(workgroup: &RawWorkgroup) -> usize {
    match *workgroup {}
}

pub(crate) fn workgroup_max_parallel_threads(workgroup: &RawWorkgroup) -> Option<u32> {
    match *workgroup {}
}

pub(crate) fn workgroup_join(workgroup: &RawWorkgroup) -> Result<RawJoinToken, i32> {
    match *workgroup {}
}

pub(crate) fn workgroup_leave(workgroup: &RawWorkgroup, _token: RawJoinToken) {
    match *workgroup {}
}

pub(crate) fn work_interval_start(
    workgroup: &RawWorkgroup,
    _length: std::time::Duration,
) -> Result<(), i32> {
    match *workgroup {}
}

pub(crate) fn work_interval_finish(workgroup: &RawWorkgroup) -> Result<(), i32> {
    match *workgroup {}
}
