//! Compiles the vendored Ableton Link 4.1.1 and its C interface (`abl_link`)
//! with the `cc` crate (the Link spec, R12, R16).
//!
//! Link and standalone asio are vendored under `vendor/`, with their licences
//! and provenance in that folder's README.md, so the build downloads nothing
//! and needs no `bindgen` and no libclang.

fn main() {
    let link = std::path::Path::new("vendor/link");
    let asio = std::path::Path::new("vendor/asio");
    let target = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let mut build = cc::Build::new();
    build
        .cpp(true)
        .std("c++17")
        .file(link.join("abl_link.cpp"))
        .include(link)
        .include(asio)
        .define("ASIO_STANDALONE", "1")
        .define("ASIO_NO_TYPEID", "1")
        // Beatlayer's own inline namespace, so another copy of asio in the
        // same program doesn't clash with this one.
        .define("ASIO_VERSION_NAMESPACE", "bl_link_asio_1_38_2")
        // It is upstream's code: its warnings aren't ours to fix.
        .warnings(false)
        .opt_level(2);
    match target.as_str() {
        "macos" | "ios" => {
            build.define("LINK_PLATFORM_UNIX", "1");
            build.define("LINK_PLATFORM_MACOSX", "1");
        }
        "windows" => {
            build.define("LINK_PLATFORM_WINDOWS", "1");
            build.define("INCL_EXTRA_HTON_FUNCTIONS", "1");
            build.define("_WIN32_WINNT", "0x0601");
        }
        _ => {
            build.define("LINK_PLATFORM_UNIX", "1");
            build.define("LINK_PLATFORM_LINUX", "1");
        }
    }
    build.compile("bl_link_abl");
    match target.as_str() {
        "windows" => {
            for lib in ["ws2_32", "iphlpapi", "mswsock"] {
                println!("cargo:rustc-link-lib={lib}");
            }
        }
        "linux" => {
            println!("cargo:rustc-link-lib=atomic");
            println!("cargo:rustc-link-lib=pthread");
        }
        _ => {}
    }
    println!("cargo:rerun-if-changed=vendor/link/abl_link.cpp");
    println!("cargo:rerun-if-changed=vendor/link/abl_link.h");
    println!("cargo:rerun-if-changed=build.rs");
}
