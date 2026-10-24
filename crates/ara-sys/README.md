# bl-ara-sys

The ARA 2 C API (Celemony's Audio Random Access) as Rust types: the structs, constants and function tables of the SDK's `ARAInterface.h`. Nothing is fetched or compiled at build time.

The SDK is Apache-2.0; its licence text is `LICENSE-APACHE-2.0.txt`, and this crate's `license` field names it so `cargo deny` checks it against the allow-list. The bindings are generated code derived from `ARAInterface.h` (copyright Celemony Software GmbH, Apache-2.0).

## The two files

The header packs its structs by target: on x86 it packs to 1 byte, on ARM to 8. So there is one file for each, picked by `target_arch`:

- `src/bindings_x86_64.rs`
- `src/bindings_aarch64.rs`

## Regenerating them

They are generated once, from ARA SDK 2.1.0 (tag `releases/2.1.0`, with the `ARA_API` submodule), with `bindgen-cli` 0.73.2 and the SDK cloned outside this repository:

```sh
for target in x86_64 aarch64; do
  bindgen ARA_API/ARAInterface.h --no-layout-tests --use-core --no-doc-comments \
    --allowlist-file '.*ARAInterface\.h' --default-enum-style consts \
    --no-prepend-enum-name --disable-header-comment --no-derive-debug \
    --rust-target 1.82 -o bindings_$target.rs -- --target=$target-apple-darwin -DARA_MAC=1
done
```

Then each anonymous enum's type (`_bindgen_ty_N`) is replaced by `i32`, or `usize` for the `…MinSize` constants, and its alias line is dropped.

The VST3 and Audio Unit companion headers are C++ (`ARAVST3.h`) or need Apple's headers (`ARAAudioUnit.h`): the host's VST3 side (interface IDs and vtables) is written by hand in `bl-plugin-host`.
