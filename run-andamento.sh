#!/usr/bin/env bash
set -euo pipefail
export ANDAMENTO_ROOT=${ANDAMENTO_ROOT:-/Users/robert/dev/andamento}
cargo build --manifest-path "$ANDAMENTO_ROOT/Cargo.toml" --workspace --target wasm32-wasip1 --release
ZELLIJ_BIN=${ZELLIJ_BIN:-/Users/robert/dev/zellij/target/dev-opt/zellij}
exec "$ZELLIJ_BIN" --layout "$ANDAMENTO_ROOT/layouts/andamento.kdl"
