#!/bin/sh
# Regenerate crates/andamento-ffi/include/andamento.h from the crate's
# source with cbindgen (`cargo install cbindgen`). With --check, fail if
# the committed header is stale instead.
set -e
ffi="$(dirname "$0")/../crates/andamento-ffi"
out="$ffi/include/andamento.h"
generate() {
  cbindgen --quiet --config "$ffi/cbindgen.toml" --output "$1" "$ffi/src/lib.rs"
}
if [ "$1" = "--check" ]; then
  tmp=$(mktemp)
  trap 'rm -f "$tmp"' EXIT
  generate "$tmp"
  if ! diff -u "$out" "$tmp"; then
    echo "andamento.h is stale: run scripts/generate-header.sh" >&2
    exit 1
  fi
else
  generate "$out"
fi
