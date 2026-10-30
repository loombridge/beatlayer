# Ableton Link and asio, vendored

This folder holds the C++ sources of [Ableton Link](https://github.com/Ableton/link)
4.1.1 and of the standalone [asio](https://github.com/chriskohlhoff/asio) it
builds against, with Link's own C interface (`abl_link.h`, `abl_link.cpp`).
`bl-link` is built on them (the Link spec, R12 and R16).

They are vendored rather than taken as crates so that the build needs no
`bindgen` and no libclang, and so that no build script downloads anything:
`crates/link/build.rs` compiles `link/abl_link.cpp` with the `cc` crate and
links it into `bl-link`, whose `src/ffi.rs` declares the functions by hand.

## What is here, and where it came from

| Path | Upstream | Revision |
|---|---|---|
| `link/ableton/` (the headers, with upstream's test headers left out), `link/abl_link.h`, `link/abl_link.cpp` | `github.com/Ableton/link`, `include/` and `extensions/abl_link/` | tag `Link-4.1.1`, `5ba907ae027a8c2ec8824463e37296dcefc5e9be` |
| `link/LICENSE.md`, `link/GNU-GPL-v2.0.md` | the same repository | as above |
| `asio/asio.hpp`, `asio/asio/` | `github.com/chriskohlhoff/asio` (Link's `modules/asio-standalone`), `asio/include/` | `8806a6803cde7054c3049d3666d3ec36786568c5` (asio 1.38.2) |
| `asio/LICENSE_1_0.txt` | the same repository | as above |

Nothing else was taken: upstream's examples, tests, CMake files and the other
extensions are left out.

## Licences

- Link is GPL-2.0-or-later, with its notice in `link/LICENSE.md`. Ableton
  offers a proprietary licence for software that doesn't take the GPL; Beatlayer
  is GPL-licensed, so it uses Link under it.
- asio is under the Boost Software License 1.0 (BSL-1.0), in
  `asio/LICENSE_1_0.txt`.

## How it is built

`build.rs` defines what Link's CMake files do: `LINK_PLATFORM_UNIX`,
`LINK_PLATFORM_MACOSX` or `LINK_PLATFORM_LINUX` or `LINK_PLATFORM_WINDOWS`,
`ASIO_STANDALONE`, `ASIO_NO_TYPEID`, and an `ASIO_VERSION_NAMESPACE` of
Beatlayer's own, `bl_link_asio_1_38_2`. On Windows it also defines
`_WIN32_WINNT` and `INCL_EXTRA_HTON_FUNCTIONS` and links the socket
libraries; on Linux it links `atomic` and `pthread`.

Nothing in this folder is edited: to move to a newer revision, replace the
files and update the table above and the namespace in `build.rs`.
