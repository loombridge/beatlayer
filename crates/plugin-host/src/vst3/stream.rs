//! The stream the host hands a VST3 plug-in to save a state into or load
//! one from (VST3's `IBStream`): a byte buffer the host owns, touched only
//! on the main thread.

use std::ffi::c_void;
use std::ptr;
use std::sync::{Arc, Mutex, PoisonError};

use vst3::Steinberg::*;
use vst3::{Class, ComWrapper};

use super::Vst3Error;

/// A state's bytes, and where the plug-in is in them.
#[derive(Default)]
struct Bytes {
    data: Vec<u8>,
    pos: usize,
}

/// The host's stream: what a plug-in's `getState`/`setState` reads or
/// writes through.
pub(crate) struct HostStream {
    bytes: Arc<Mutex<Bytes>>,
    /// Whether the plug-in is writing a state into it.
    writing: bool,
}

fn lock(bytes: &Mutex<Bytes>) -> std::sync::MutexGuard<'_, Bytes> {
    bytes.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Class for HostStream {
    type Interfaces = (IBStream,);
}

impl IBStreamTrait for HostStream {
    unsafe fn read(
        &self,
        buffer: *mut c_void,
        num_bytes: int32,
        num_bytes_read: *mut int32,
    ) -> tresult {
        if buffer.is_null() || num_bytes_read.is_null() || self.writing {
            return kInvalidArgument;
        }
        let mut bytes = lock(&self.bytes);
        let at = bytes.pos.min(bytes.data.len());
        let want = usize::try_from(num_bytes.max(0))
            .unwrap_or(0)
            .min(bytes.data.len() - at);
        // SAFETY: the caller passes a buffer of `num_bytes`, as the
        // interface says it does; only `want` bytes of it are written.
        unsafe {
            ptr::copy_nonoverlapping(bytes.data.as_ptr().add(at), buffer.cast::<u8>(), want);
            *num_bytes_read = int32::try_from(want).unwrap_or(int32::MAX);
        }
        bytes.pos = at + want;
        if want == 0 && num_bytes > 0 {
            // The end: a plug-in reading in a loop stops on this, as it
            // does on the SDK's own memory stream.
            kResultFalse
        } else {
            kResultOk
        }
    }

    unsafe fn write(
        &self,
        buffer: *mut c_void,
        num_bytes: int32,
        num_bytes_written: *mut int32,
    ) -> tresult {
        if buffer.is_null() || num_bytes_written.is_null() || !self.writing {
            return kInvalidArgument;
        }
        let want = usize::try_from(num_bytes.max(0)).unwrap_or(0);
        let mut bytes = lock(&self.bytes);
        let pos = bytes.pos;
        if pos > bytes.data.len() {
            bytes.data.resize(pos, 0);
        }
        let end = pos.saturating_add(want);
        if bytes.data.len() < end {
            bytes.data.resize(end, 0);
        }
        // SAFETY: the caller passes a buffer of `num_bytes`, as the
        // interface says it does.
        unsafe {
            ptr::copy_nonoverlapping(buffer.cast::<u8>(), bytes.data.as_mut_ptr().add(pos), want);
            *num_bytes_written = int32::try_from(want).unwrap_or(int32::MAX);
        }
        bytes.pos = end;
        kResultOk
    }

    unsafe fn seek(&self, pos: int64, mode: int32, result: *mut int64) -> tresult {
        use IBStream_::IStreamSeekMode_ as Mode;
        let mode = u32::from_ne_bytes(mode.to_ne_bytes());
        let mut bytes = lock(&self.bytes);
        let base = match mode {
            x if x == Mode::kIBSeekSet => 0i64,
            x if x == Mode::kIBSeekCur => i64::try_from(bytes.pos).unwrap_or(i64::MAX),
            x if x == Mode::kIBSeekEnd => i64::try_from(bytes.data.len()).unwrap_or(i64::MAX),
            _ => return kInvalidArgument,
        };
        let target = base.saturating_add(pos).max(0);
        bytes.pos = usize::try_from(target).unwrap_or(usize::MAX);
        if !result.is_null() {
            // SAFETY: the caller passes a valid pointer or null, as the
            // interface says it does.
            unsafe { *result = target };
        }
        kResultOk
    }

    unsafe fn tell(&self, pos: *mut int64) -> tresult {
        if pos.is_null() {
            return kInvalidArgument;
        }
        let bytes = lock(&self.bytes);
        // SAFETY: the caller passes a valid pointer, as the interface says
        // it does.
        unsafe { *pos = i64::try_from(bytes.pos).unwrap_or(i64::MAX) };
        kResultOk
    }
}

/// Saves a state: calls `f` with a stream the plug-in writes into, and
/// gives back the bytes it wrote. `Err` if the plug-in refused.
pub(crate) fn save(f: impl FnOnce(*mut IBStream) -> tresult) -> Result<Vec<u8>, Vst3Error> {
    let bytes = Arc::new(Mutex::new(Bytes::default()));
    let stream = HostStream {
        bytes: Arc::clone(&bytes),
        writing: true,
    };
    let wrapper = ComWrapper::new(stream);
    let refused = Vst3Error::Refused("to save its state".to_owned());
    let Some(stream) = wrapper.as_com_ref::<IBStream>() else {
        return Err(refused);
    };
    if f(stream.as_ptr()) != kResultOk {
        return Err(refused);
    }
    let saved = lock(&bytes).data.clone();
    Ok(saved)
}

/// Loads a state: calls `f` with a stream over `data`.
pub(crate) fn load(data: &[u8], f: impl FnOnce(*mut IBStream) -> tresult) -> tresult {
    let stream = HostStream {
        bytes: Arc::new(Mutex::new(Bytes {
            data: data.to_vec(),
            pos: 0,
        })),
        writing: false,
    };
    let wrapper = ComWrapper::new(stream);
    let Some(stream) = wrapper.as_com_ref::<IBStream>() else {
        return kNotImplemented;
    };
    f(stream.as_ptr())
}
