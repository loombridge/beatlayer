# Fuzz targets

Fuzz targets for Beatlayer's file readers, run with [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz) and libFuzzer. This folder is its own Cargo workspace with its own `Cargo.lock`, outside the product's workspace like `spikes/`, because cargo-fuzz needs a nightly toolchain and the product builds on the pinned stable one.

| Target | What it feeds | Checks |
|---|---|---|
| `peak_file` | Bytes as read from a `.blpeaks` file: magic, header, run table, data block, and BLAKE3 checksum | No panic. Anything the reader accepts saves again and loads back as the same peaks. |
| `preset` | Bytes as read from a `.blpreset` file: JSON format header, schema version, device info, and parameters | No panic. Anything the decoder accepts saves again and decodes to the same preset. |
| `set_file` | Bytes as read from a `.blset` file: zstd, its checksum, the JSON, migrations, and the document model's rules | No panic. Anything the loader accepts saves again, loads back as the same song, and gives an editor that hands out an unused ID. |
| `set_json` | The JSON inside a set, skipping the compression and checksum that random bytes rarely get through | The same |

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
cargo +nightly fuzz run peak_file corpus/peak_file -- \
  -max_total_time=300
cargo +nightly fuzz run preset corpus/preset -- \
  -max_total_time=300
```

The first folder after the target is the working corpus, which libFuzzer grows; the fixtures are read-only seeds. Corpora, crash artefacts, and build output stay out of Git (`.gitignore`). A crash writes its input to `artifacts/<target>/`; turn it into a regression test in `crates/project` before fixing it.

The loader's JSON limit is 8 MiB under fuzzing (`OPTIONS` in `src/lib.rs`), so a decompression bomb stops quickly instead of exhausting libFuzzer's 2 GB memory limit.
