#!/usr/bin/env bash
set -euo pipefail
cd /Users/robert/dev/zellij-scratch
cargo build --release
ZELLIJ_BIN=${ZELLIJ_BIN:-/Users/robert/dev/zellij/target/dev-opt/zellij}
exec "$ZELLIJ_BIN" --config /Users/robert/dev/zellij-scratch/try-vertical-tabs.kdl --layout /Users/robert/dev/zellij-scratch/vertical-tabs-layout.kdl
