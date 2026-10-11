#!/bin/sh
# Regenerate crates/andamento-ffi/include/andamento_documents.h from
# src/documents.rs with cbindgen (`cargo install cbindgen`). With --check,
# fail if the committed header is stale instead.
set -e
ffi="$(dirname "$0")/../crates/andamento-ffi"
out="$ffi/include/andamento_documents.h"
if [ "$1" = "--check" ]; then
  tmp=$(mktemp)
  cbindgen --quiet --config "$ffi/cbindgen.toml" --output "$tmp" "$ffi/src/documents.rs"
  diff -u "$out" "$tmp" && rm "$tmp"
else
  cbindgen --quiet --config "$ffi/cbindgen.toml" --output "$out" "$ffi/src/documents.rs"
fi
