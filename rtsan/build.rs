//! Links the RealtimeSanitizer runtime that RTSAN_LIB_PATH names into every
//! test binary. It never downloads or builds one: without the variable, the
//! build stops and says how to get one.

use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo::rerun-if-env-changed=RTSAN_LIB_PATH");
    let Some(runtime) = env::var_os("RTSAN_LIB_PATH").map(PathBuf::from) else {
        panic!(
            "RTSAN_LIB_PATH isn't set. Run `cargo xtask rtsan`, which finds an installed \
             RealtimeSanitizer runtime, or set RTSAN_LIB_PATH to one: \
             libclang_rt.rtsan_osx_dynamic.dylib from Homebrew's llvm on macOS, or \
             libclang_rt.rtsan*.a from LLVM 20 or later on Linux (libclang-rt-20-dev from \
             apt.llvm.org). This build never downloads one."
        );
    };
    assert!(
        runtime.is_file(),
        "RTSAN_LIB_PATH names {}, which isn't a file",
        runtime.display()
    );
    println!("cargo::rerun-if-changed={}", runtime.display());
    let dir = runtime
        .parent()
        .expect("a file has a parent directory")
        .display();

    match env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("macos") => {
            // The dynamic runtime, linked where it's installed. Nothing copies
            // or edits it, so its code signature stays valid; the rpath finds
            // it at run time if its install name is relative.
            println!("cargo::rustc-link-arg={}", runtime.display());
            println!("cargo::rustc-link-arg=-Wl,-rpath,{dir}");
        }
        Ok("linux") => {
            // The static runtime, linked whole as clang's -fsanitize=realtime
            // links it, so its interceptors replace libc's functions in the
            // whole program, followed by what it needs from the system.
            println!("cargo::rustc-link-arg=-Wl,--whole-archive");
            println!("cargo::rustc-link-arg={}", runtime.display());
            println!("cargo::rustc-link-arg=-Wl,--no-whole-archive");
            let symbols = runtime.with_extension("a.syms");
            if symbols.is_file() {
                // Exports the interceptors, so calls from shared libraries
                // reach them too, as clang does with the same list.
                println!(
                    "cargo::rustc-link-arg=-Wl,--dynamic-list={}",
                    symbols.display()
                );
            }
            for library in ["pthread", "rt", "m", "dl", "resolv"] {
                println!("cargo::rustc-link-arg=-l{library}");
            }
        }
        other => panic!("RealtimeSanitizer runs on macOS and Linux, not on {other:?}"),
    }
}
