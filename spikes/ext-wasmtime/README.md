# Extension spike: wasmtime on a real-time thread (WP-9.28)

A throwaway spike, outside the product's workspace like `spikes/ui-egui`, with its own `Cargo.lock`. It answers whether WebAssembly components under `wasmtime`, with memory reserved up front, can be called for processing on an audio thread: what a call costs at 32-frame blocks, whether anything allocates after instantiation, how fast a runaway call is stopped, what a trap does, how long compiling and loading take, and what wasmtime adds to a binary. Nothing here is product code.

## What's in it

- `src/guest.rs`: the test guest, written as WebAssembly text so no wasm toolchain is needed. Four feedback combs with reverb-sized delay lines (158 KiB), in three shapes: a component taking and returning `list<f32>` through the canonical ABI, the same code as a core module the host writes into directly, and a component call that does nothing. Also a spin loop and two traps (`unreachable` and an out-of-bounds load). `Reference` computes the same sums in Rust, so a test compares the guest bit for bit.
- `src/host.rs`: the engine (pooling allocator, epoch interruption), the ticker thread, and the guest instances.
- `src/scenario.rs`: the measurements. `src/main.rs` prints them as Markdown.
- `src/alloc.rs`: a counting global allocator. `src/rt.rs`: a real-time section that is RealtimeSanitizer's when the spike is built with a runtime, and nothing otherwise.
- `tests/`: the acceptance points as tests.

## Running it

```sh
cargo test --release                  # the tests
cargo run --release                   # every measurement, as Markdown
cargo run --release -- calls --calls 100000
cargo run --release -- runaway traps compile
scripts/sizes.sh                      # binary sizes, one build per feature set
```

Under RealtimeSanitizer, point `RTSAN_LIB_PATH` at an installed runtime (`libclang_rt.rtsan_osx_dynamic.dylib` from Homebrew's llvm, or `libclang_rt.rtsan*.a` from LLVM 20 or later) and build into a target directory of its own:

```sh
RTSAN_LIB_PATH=... cargo test --release --target-dir target/rtsan
```

`build.rs` links the runtime it names and never downloads one. `.github/workflows/ext-spike.yml` runs all of this on macOS, Windows and Linux (RealtimeSanitizer on macOS and Linux only), and `cargo deny` over the dependency tree on Linux.

The wasmtime features are the ones the product would want: no `cache` (its dependency on `zstd` fails the repository's advisory check today), no async, GC, profiling or debug support.
