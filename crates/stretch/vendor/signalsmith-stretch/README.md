# Signalsmith Stretch, vendored

This folder holds the C++ sources of [Signalsmith Stretch](https://signalsmith-audio.co.uk/code/stretch/)
and of the [Signalsmith Linear](https://github.com/Signalsmith-Audio/linear) library it draws its
FFTs from, together with Beatlayer's own small C wrapper over them
(`bl_stretch.h`, `bl_stretch.cpp`). The Complex and Complex Pro warp modes are
built on it (the architecture's decision D11; the warp modes spec, R21 to R24).

It is vendored rather than taken as a crate so that the build needs no
`bindgen` and no libclang, and so that no build script downloads anything:
`crates/stretch/build.rs` compiles `bl_stretch.cpp` with the `cc` crate and
links it into `bl-stretch`.

## What is here, and where it came from

| Path | Upstream | Revision |
|---|---|---|
| `signalsmith-stretch.h` | `github.com/Signalsmith-Audio/signalsmith-stretch` | `a670068d9aeb64913331d5cc29337b19a457a7df` (25 September 2026) |
| `LICENSE.txt` | the same repository | as above |
| `signalsmith-linear/stft.h`, `fft.h`, `linear.h`, `approx.h`, `platform/fft-linear.h` | `github.com/Signalsmith-Audio/linear` | `146b26f91b48835d665fa0498d64d9617818d27a` (4 October 2026) |
| `signalsmith-linear/LICENSE.txt` | the same repository | as above |

`signalsmith-stretch.h` includes `signalsmith-linear/stft.h`; the rest of the
Linear headers are what that one includes in turn. Nothing else was taken: the
upstream command-line tools, the web build and the tests are left out.

Both libraries are MIT-licensed, with their notices in the `LICENSE.txt` files
beside them.

## Beatlayer's own files

- `bl_stretch.h`, `bl_stretch.cpp`: a C interface to `SignalsmithStretch<float>`
  and the pointer arrays it indexes, so the Rust side calls plain functions and
  allocates nothing per call. Every call is a straight pass-through; the wrapper
  adds no behaviour. Read them for the interface.

Nothing in this folder is edited: to move to a newer revision, replace the
files and update the table above.
