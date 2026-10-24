//! ARA's two VST3 entry-point interfaces (`ARAVST3.h`), as COM interfaces a
//! Rust class can implement: `IPlugInEntryPoint`, which gives the plug-in's
//! ARA factory, and `IPlugInEntryPoint2`, which binds an instance to a
//! document controller with roles. The `vst3` crate generates the VST3
//! SDK's interfaces from its headers; ARA's two are written by hand in the
//! same shape.

use std::ffi::c_void;
use std::ptr;

use bl_ara_sys as ara;
use vst3::Steinberg::{FUnknown, FUnknownVtbl, TUID, kResultOk};
use vst3::com_scrape_types::{Class, Construct, Guid, Inherits, Interface, Unknown, Wrapper};

/// Byte `at` (from the top) of a 32-bit word.
const fn b(value: u32, at: u32) -> u8 {
    (value >> (24 - 8 * at)) as u8
}

/// A class ID as the VST3 SDK's `INLINE_UID` writes it.
const fn tuid(l1: u32, l2: u32, l3: u32, l4: u32) -> Guid {
    if cfg!(windows) {
        [
            l1 as u8,
            (l1 >> 8) as u8,
            (l1 >> 16) as u8,
            (l1 >> 24) as u8,
            (l2 >> 16) as u8,
            (l2 >> 24) as u8,
            l2 as u8,
            (l2 >> 8) as u8,
            b(l3, 0),
            b(l3, 1),
            b(l3, 2),
            b(l3, 3),
            b(l4, 0),
            b(l4, 1),
            b(l4, 2),
            b(l4, 3),
        ]
    } else {
        [
            b(l1, 0),
            b(l1, 1),
            b(l1, 2),
            b(l1, 3),
            b(l2, 0),
            b(l2, 1),
            b(l2, 2),
            b(l2, 3),
            b(l3, 0),
            b(l3, 1),
            b(l3, 2),
            b(l3, 3),
            b(l4, 0),
            b(l4, 1),
            b(l4, 2),
            b(l4, 3),
        ]
    }
}

unsafe fn query(this: *mut c_void, iid: &Guid) -> Option<*mut c_void> {
    let unknown = this.cast::<FUnknown>();
    let mut object = ptr::null_mut();
    // SAFETY: a live COM object asked for an interface by ID.
    let result = unsafe {
        ((*(*unknown).vtbl).queryInterface)(unknown, iid.as_ptr().cast::<TUID>(), &mut object)
    };
    (result == kResultOk).then_some(object)
}

unsafe fn add_ref(this: *mut c_void) -> usize {
    let unknown = this.cast::<FUnknown>();
    // SAFETY: as above.
    unsafe { ((*(*unknown).vtbl).addRef)(unknown) as usize }
}

unsafe fn release(this: *mut c_void) -> usize {
    let unknown = this.cast::<FUnknown>();
    // SAFETY: as above.
    unsafe { ((*(*unknown).vtbl).release)(unknown) as usize }
}

/// What every one of ARA's interfaces shares: its type, and its `Unknown`
/// and `Interface` implementations.
macro_rules! interface {
    ($name:ident, $vtbl:ident, $iid:expr) => {
        #[repr(C)]
        #[derive(Copy, Clone)]
        pub struct $name {
            pub vtbl: *const $vtbl,
        }

        // SAFETY: a table of function pointers.
        unsafe impl Send for $name {}
        // SAFETY: as above.
        unsafe impl Sync for $name {}
        // SAFETY: the table starts with `FUnknown`'s.
        unsafe impl Inherits<FUnknown> for $name {}

        impl Unknown for $name {
            unsafe fn query_interface(this: *mut Self, iid: &Guid) -> Option<*mut c_void> {
                // SAFETY: the contract of `Unknown`.
                unsafe { query(this.cast(), iid) }
            }

            unsafe fn add_ref(this: *mut Self) -> usize {
                // SAFETY: as above.
                unsafe { add_ref(this.cast()) }
            }

            unsafe fn release(this: *mut Self) -> usize {
                // SAFETY: as above.
                unsafe { release(this.cast()) }
            }
        }

        // SAFETY: the object is its table pointer, and the table starts
        // with `FUnknown`'s.
        unsafe impl Interface for $name {
            type Vtbl = $vtbl;
            const IID: Guid = $iid;

            fn inherits(iid: &Guid) -> bool {
                iid == &Self::IID || FUnknown::inherits(iid)
            }
        }
    };
}

interface!(
    IPlugInEntryPoint,
    IPlugInEntryPointVtbl,
    tuid(0x1281_4E54, 0xA1CE_4076, 0x82B9_6813, 0x1695_0BD6)
);
interface!(
    IPlugInEntryPoint2,
    IPlugInEntryPoint2Vtbl,
    tuid(0xCD9A_5913, 0xC9EB_46D7, 0x96CA_53AD, 0xD1DB_89F5)
);

#[repr(C)]
#[derive(Copy, Clone)]
pub struct IPlugInEntryPointVtbl {
    pub base: FUnknownVtbl,
    pub get_factory:
        unsafe extern "system" fn(this: *mut IPlugInEntryPoint) -> *const ara::ARAFactory,
    pub bind_to_document_controller:
        unsafe extern "system" fn(
            this: *mut IPlugInEntryPoint,
            document: ara::ARADocumentControllerRef,
        ) -> *const ara::ARAPlugInExtensionInstance,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct IPlugInEntryPoint2Vtbl {
    pub base: FUnknownVtbl,
    pub bind_to_document_controller_with_roles:
        unsafe extern "system" fn(
            this: *mut IPlugInEntryPoint2,
            document: ara::ARADocumentControllerRef,
            known: ara::ARAPlugInInstanceRoleFlags,
            assigned: ara::ARAPlugInInstanceRoleFlags,
        ) -> *const ara::ARAPlugInExtensionInstance,
}

pub trait IPlugInEntryPointTrait {
    unsafe fn get_factory(&self) -> *const ara::ARAFactory;
    unsafe fn bind_to_document_controller(
        &self,
        document: ara::ARADocumentControllerRef,
    ) -> *const ara::ARAPlugInExtensionInstance;
}

pub trait IPlugInEntryPoint2Trait {
    unsafe fn bind_to_document_controller_with_roles(
        &self,
        document: ara::ARADocumentControllerRef,
        known: ara::ARAPlugInInstanceRoleFlags,
        assigned: ara::ARAPlugInInstanceRoleFlags,
    ) -> *const ara::ARAPlugInExtensionInstance;
}

impl IPlugInEntryPoint {
    const fn make_vtbl<C, W, const OFFSET: isize>() -> IPlugInEntryPointVtbl
    where
        C: IPlugInEntryPointTrait + Class,
        W: Wrapper<C>,
    {
        unsafe extern "system" fn get_factory<C, W, const OFFSET: isize>(
            this: *mut IPlugInEntryPoint,
        ) -> *const ara::ARAFactory
        where
            C: IPlugInEntryPointTrait + Class,
            W: Wrapper<C>,
        {
            // SAFETY: the wrapper's header layout, as the generated tables do.
            unsafe {
                let header = this.cast::<u8>().offset(-OFFSET).cast();
                (*W::data_from_header(header)).get_factory()
            }
        }

        unsafe extern "system" fn bind<C, W, const OFFSET: isize>(
            this: *mut IPlugInEntryPoint,
            document: ara::ARADocumentControllerRef,
        ) -> *const ara::ARAPlugInExtensionInstance
        where
            C: IPlugInEntryPointTrait + Class,
            W: Wrapper<C>,
        {
            // SAFETY: as above.
            unsafe {
                let header = this.cast::<u8>().offset(-OFFSET).cast();
                (*W::data_from_header(header)).bind_to_document_controller(document)
            }
        }

        IPlugInEntryPointVtbl {
            base: FUnknown::make_vtbl::<C, W, OFFSET>(),
            get_factory: get_factory::<C, W, OFFSET>,
            bind_to_document_controller: bind::<C, W, OFFSET>,
        }
    }
}

// SAFETY: the object is a table pointer to a table that starts with
// `FUnknown`'s.
unsafe impl<C, W, const OFFSET: isize> Construct<C, W, OFFSET> for IPlugInEntryPoint
where
    C: IPlugInEntryPointTrait + Class,
    W: Wrapper<C>,
{
    const OBJ: Self = Self {
        vtbl: &Self::make_vtbl::<C, W, OFFSET>(),
    };
}

impl IPlugInEntryPoint2 {
    const fn make_vtbl<C, W, const OFFSET: isize>() -> IPlugInEntryPoint2Vtbl
    where
        C: IPlugInEntryPoint2Trait + Class,
        W: Wrapper<C>,
    {
        unsafe extern "system" fn bind<C, W, const OFFSET: isize>(
            this: *mut IPlugInEntryPoint2,
            document: ara::ARADocumentControllerRef,
            known: ara::ARAPlugInInstanceRoleFlags,
            assigned: ara::ARAPlugInInstanceRoleFlags,
        ) -> *const ara::ARAPlugInExtensionInstance
        where
            C: IPlugInEntryPoint2Trait + Class,
            W: Wrapper<C>,
        {
            // SAFETY: as above.
            unsafe {
                let header = this.cast::<u8>().offset(-OFFSET).cast();
                (*W::data_from_header(header))
                    .bind_to_document_controller_with_roles(document, known, assigned)
            }
        }

        IPlugInEntryPoint2Vtbl {
            base: FUnknown::make_vtbl::<C, W, OFFSET>(),
            bind_to_document_controller_with_roles: bind::<C, W, OFFSET>,
        }
    }
}

// SAFETY: as above.
unsafe impl<C, W, const OFFSET: isize> Construct<C, W, OFFSET> for IPlugInEntryPoint2
where
    C: IPlugInEntryPoint2Trait + Class,
    W: Wrapper<C>,
{
    const OBJ: Self = Self {
        vtbl: &Self::make_vtbl::<C, W, OFFSET>(),
    };
}
