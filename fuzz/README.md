# Fuzz targets

Fuzz targets for Beatlayer's file readers, run with [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz) and libFuzzer. This folder is its own Cargo workspace with its own `Cargo.lock`, outside the product's workspace like `spikes/`, because cargo-fuzz needs a nightly toolchain and the product builds on the pinned stable one.

| Target | What it feeds | Checks |
|---|---|---|
| `audio_header` | Bytes as read from an audio file of any format Beatlayer imports: the format probe, the header readers, and the limits on what a header may declare, then the first 4096 frames | No panic. Anything opened declares no more than `bl_media::limits` allows. |
| `dawproject` | Bytes as read from a `.dawproject` container, and `project.xml` alone: the ZIP reader, the XML reader and the import | No panic. Whatever the import makes obeys the document model's rules. |
| `groove_file` | Bytes as read from a `.blgroove` file: JSON format header, schema version, notes, parameters, and the grooves spec's limits | No panic. Anything the decoder accepts saves again and decodes to the same groove. |
| `midi_file` | Bytes as read from a `.mid` or RMID file: the size cap, header and track chunks, events, tempo and time signatures | No panic. Every note has a length, a key and velocity in range, ends within its track, and comes in order. |
| `peak_file` | Bytes as read from a `.blpeaks` file: magic, header, run table, data block, and BLAKE3 checksum | No panic. Anything the reader accepts saves again and loads back as the same peaks. |
| `preset` | Bytes as read from a `.blpreset` file: JSON format header, schema version, device info, and parameters | No panic. Anything the decoder accepts saves again and decodes to the same preset. |
| `sfz` | Bytes as read from an `.sfz` file: comments, `#define`, headers and opcodes (no `#include` files) | No panic. Every region it reads has zones, a detune and a pan inside the layers' ranges. |
| `set_file` | Bytes as read from a `.blset` file: zstd, its checksum, the JSON, migrations, and the document model's rules | No panic. Anything the loader accepts saves again, loads back as the same song, and gives an editor that hands out an unused ID. |
| `set_json` | The JSON inside a set, skipping the compression and checksum that random bytes rarely get through | The same |
| `sfz` | Bytes as read from an `.sfz` file: comments, `#define`s, headers and opcodes | No panic. Every region keeps its key, velocity, detune and pan ranges. |

## In CI

The Fuzz workflow (`.github/workflows/fuzz.yml`) runs every target listed in `Cargo.toml` for 15 minutes each, every Monday and on demand from the Actions tab, seeded and with dictionaries as below. A crash fails the run and is uploaded as the artefact `fuzz-crash-<target>`. A new target needs no change to the workflow, unless it takes seeds from somewhere: add those to its Fuzz step.

## Run them

You need a nightly toolchain and cargo-fuzz once:

```sh
rustup toolchain install nightly --profile minimal
cargo install --locked cargo-fuzz
```

Then, from this folder, seed each target with the schema fixtures and run it:

```sh
cd fuzz
cargo +nightly fuzz run set_json corpus/set_json ../crates/project/tests/fixtures -- \
  -dict=dictionaries/set_json.dict -max_total_time=600
cargo +nightly fuzz run set_file corpus/set_file ../crates/project/tests/fixtures -- \
  -max_total_time=600
cargo +nightly fuzz run audio_header corpus/audio_header -- \
  -dict=dictionaries/audio_header.dict -max_total_time=300
cargo +nightly fuzz run midi_file corpus/midi_file -- \
  -dict=dictionaries/midi_file.dict -max_total_time=300
cargo +nightly fuzz run peak_file corpus/peak_file -- \
  -max_total_time=300
cargo +nightly fuzz run preset corpus/preset -- \
  -max_total_time=300
cargo +nightly fuzz run sfz corpus/sfz -- \
  -max_total_time=300
```

The first folder after the target is the working corpus, which libFuzzer grows; the fixtures are read-only seeds. Corpora, crash artefacts, and build output stay out of Git (`.gitignore`). A crash writes its input to `artifacts/<target>/`; turn it into a regression test in `crates/project` before fixing it.

The loader's JSON limit is 8 MiB under fuzzing (`OPTIONS` in `src/lib.rs`), so a decompression bomb stops quickly instead of exhausting libFuzzer's 2 GB memory limit.
