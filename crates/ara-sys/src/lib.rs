//! The ARA 2 C API's types and function tables (see the README for where
//! they come from). Names are the SDK's own, so the SDK's documentation
//! applies to them as written.

#![allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    clippy::all,
    clippy::pedantic,
    missing_debug_implementations,
    unsafe_op_in_unsafe_fn
)]

#[cfg(target_arch = "x86_64")]
include!("bindings_x86_64.rs");

#[cfg(target_arch = "aarch64")]
include!("bindings_aarch64.rs");

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
compile_error!("ARA's struct packing is defined for x86_64 and aarch64 only");
