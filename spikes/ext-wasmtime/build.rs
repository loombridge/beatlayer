//! Links the RealtimeSanitizer runtime that RTSAN_LIB_PATH names, if it's set,
//! and turns on `--cfg ext_rtsan`, which makes the real-time section real.
//! Never downloads or builds a runtime; without the variable the spike builds
//! without the sanitizer.

use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo::rustc-check-cfg=cfg(ext_rtsan)");
    println!("cargo::rerun-if-env-changed=RTSAN_LIB_PATH");
    let Some(runtime) = env::var_os("RTSAN_LIB_PATH").map(PathBuf::from) else {
        return;
    };
    assert!(
        runtime.is_file(),
        "RTSAN_LIB_PATH names {}, which isn't a file",
        runtime.display()
    );
    println!("cargo::rerun-if-changed={}", runtime.display());
    println!("cargo::rustc-cfg=ext_rtsan");
    let dir = runtime
        .parent()
        .expect("a file has a parent directory")
        .display();
    match env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("macos") => {
            println!("cargo::rustc-link-arg={}", runtime.display());
            println!("cargo::rustc-link-arg=-Wl,-rpath,{dir}");
        }
        Ok("linux") => {
            // The static runtime, linked whole as clang's -fsanitize=realtime
            // links it, so its interceptors replace libc's functions.
            println!("cargo::rustc-link-arg=-Wl,--whole-archive");
            println!("cargo::rustc-link-arg={}", runtime.display());
            println!("cargo::rustc-link-arg=-Wl,--no-whole-archive");
            let symbols = runtime.with_extension("a.syms");
            if symbols.is_file() {
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
