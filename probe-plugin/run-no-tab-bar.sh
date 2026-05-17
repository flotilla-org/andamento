#!/usr/bin/env bash
set -euo pipefail
cd /Users/robert/dev/zellij-scratch/probe-plugin
cargo build
exec zellij -n /Users/robert/dev/zellij-scratch/probe-plugin/test-layout-no-tab-bar.kdl
