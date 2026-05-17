#!/usr/bin/env bash
set -euo pipefail
cd /Users/robert/dev/zellij-scratch/probe-plugin
cargo build
exec zellij -l /Users/robert/dev/zellij-scratch/probe-plugin/test-layout.kdl
