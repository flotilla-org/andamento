#!/usr/bin/env bash
set -euo pipefail
cd /Users/robert/dev/zellij-scratch
cargo build --workspace --target wasm32-wasip1 --release
ZELLIJ_BIN=${ZELLIJ_BIN:-/Users/robert/dev/zellij/target/dev-opt/zellij}
exec "$ZELLIJ_BIN" --layout /Users/robert/dev/zellij-scratch/layouts/andamento.kdl
