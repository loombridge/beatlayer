#!/usr/bin/env bash
# Builds the spike's binary with no wasmtime, with the runtime only, with each
# compiler and with both, prints the stripped release sizes as a Markdown
# table, then checks that the runtime-only build loads what the full build
# precompiled. Run from spikes/ext-wasmtime.
set -euo pipefail

exe=target/release/ext-wasmtime-spike
[[ "${OS:-}" == Windows_NT ]] && exe=$exe.exe

echo "| Build | Release binary, bytes |"
echo "|---|---|"
for build in \
  "no wasmtime:--no-default-features" \
  "runtime only:--no-default-features --features runtime" \
  "Cranelift:--no-default-features --features cranelift" \
  "Winch:--no-default-features --features winch" \
  "Cranelift and Winch:"; do
  name=${build%%:*}
  flags=${build#*:}
  # shellcheck disable=SC2086
  cargo build --release --quiet $flags
  echo "| $name | $(wc -c < "$exe" | tr -d ' ') |"
done

# The last build was the full one.
cache=$(mktemp -d)
"$exe" precompile "$cache/guest.cwasm"
cargo build --release --quiet --no-default-features --features runtime
echo
echo "runtime-only build: $("$exe" "$cache/guest.cwasm")"
rm -f "$cache/guest.cwasm"
rmdir "$cache"
