//! Compiles the vendored Signalsmith Stretch C++ that the Complex and Complex
//! Pro warp modes are built on (the warp modes spec, R21).
//!
//! The library is vendored under `vendor/signalsmith-stretch/`, with its MIT
//! licences and its provenance in that folder's README.md, so the build
//! downloads nothing and needs no `bindgen` and no libclang.

fn main() {
    let vendor = std::path::Path::new("vendor/signalsmith-stretch");
    let mut build = cc::Build::new();
    build
        .cpp(true)
        .std("c++11")
        .file(vendor.join("bl_stretch.cpp"))
        .include(vendor)
        // The stretcher is a phase vocoder, and unoptimised it is far too slow
        // to play a clip in real time (its own README says up to ten times):
        // a debug or test build still has to play one, so it is compiled at
        // the same optimisation as the rest of Beatlayer's slow code.
        .opt_level(2);
    build.compile("bl_signalsmith_stretch");
    for changed in [
        "bl_stretch.cpp",
        "bl_stretch.h",
        "signalsmith-stretch.h",
        "signalsmith-linear/stft.h",
        "signalsmith-linear/fft.h",
        "signalsmith-linear/linear.h",
        "signalsmith-linear/approx.h",
        "signalsmith-linear/platform/fft-linear.h",
    ] {
        println!("cargo:rerun-if-changed={}", vendor.join(changed).display());
    }
}
