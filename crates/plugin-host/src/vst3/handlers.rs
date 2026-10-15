//! The host objects a VST3 plug-in calls: the host application it's given
//! at initialization, and the component handler its controller reports
//! parameter gestures to.

use std::ffi::c_void;

use vst3::Class;
use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;

/// The host, as `IHostApplication`: its name, and nothing it can create.
pub(crate) struct HostApplication;

impl Class for HostApplication {
    type Interfaces = (IHostApplication,);
}

impl IHostApplicationTrait for HostApplication {
    unsafe fn getName(&self, name: *mut String128) -> tresult {
        if name.is_null() {
            return kInvalidArgument;
        }
        // SAFETY: the caller passes a valid name buffer, as the interface
        // says it does.
        let buffer = unsafe { &mut *name };
        buffer.fill(0);
        for (dst, src) in buffer.iter_mut().zip("Beatlayer".encode_utf16()) {
            *dst = src;
        }
        kResultOk
    }

    unsafe fn createInstance(
        &self,
        _cid: *mut TUID,
        _iid: *mut TUID,
        _obj: *mut *mut c_void,
    ) -> tresult {
        kNotImplemented
    }
}

/// The component handler: the changes a plug-in makes itself, and its ask
/// for a restart. Part 1 records nothing: gestures and restarts into the
/// set are the next part's.
pub(crate) struct ComponentHandler;

impl Class for ComponentHandler {
    type Interfaces = (IComponentHandler,);
}

impl IComponentHandlerTrait for ComponentHandler {
    unsafe fn beginEdit(&self, _id: ParamID) -> tresult {
        kResultOk
    }

    unsafe fn performEdit(&self, _id: ParamID, _value_normalized: ParamValue) -> tresult {
        kResultOk
    }

    unsafe fn endEdit(&self, _id: ParamID) -> tresult {
        kResultOk
    }

    unsafe fn restartComponent(&self, _flags: int32) -> tresult {
        // Part 1 doesn't restart a plug-in; its part 2 will, on the flags
        // it asks with.
        kNotImplemented
    }
}
