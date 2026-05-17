# vertical-tabs

A small Zellij plugin that renders the tab list vertically.

## Build

```sh
cargo build --release
```

This produces:

```text
target/wasm32-wasip1/release/vertical-tabs.wasm
```

## Use in Zellij

Add a plugin alias to your Zellij config:

```kdl
plugins {
    vertical-tabs location="file:/Users/robert/dev/zellij-scratch/target/wasm32-wasip1/release/vertical-tabs.wasm"
}
```

Then place it in a layout, eg. as a 1-column left sidebar:

```kdl
layout {
    pane split_direction="horizontal" {
        plugin location="vertical-tabs" width=18
        children
    }
}
```

## Current behavior

- active tab is highlighted
- click a row to switch tabs
- mouse wheel switches tabs
- the visible list stays centered on the active tab when there are more tabs than rows

## Controller/Rail Prototype

This repo also contains the newer two-plugin prototype:

- `tabs-controller`: background state owner for pins, ordering, and pane statuses
- `tabs-rail`: visible sidebar renderer, intended to appear in each tab

Build both plugins:

```sh
cargo build --manifest-path /Users/robert/dev/zellij-scratch/Cargo.toml --workspace --target wasm32-wasip1 --release
```

Run with the local Zellij checkout:

```sh
cd /Users/robert/dev/zellij
cargo run --profile dev-opt -- --layout /Users/robert/dev/zellij-scratch/layouts/tabs-rail.kdl
```

Or use the scratch launcher, which defaults to the same dev binary:

```sh
/Users/robert/dev/zellij-scratch/run-vertical-tabs.sh
```

## Directory Grouping

The controller can group tabs by the exact current working directory of their panes:

```kdl
plugin location="tabs-controller" {
    rail_grouping "directory"
}
```

Use `rail_grouping "none"` or omit the setting for the flat tab rail.

Set a pane status:

```sh
zellij pipe --name tabs-set-pane-status -- '{"type":"set-pane-status","pane_id":{"kind":"terminal","id":1},"priority":"waiting","title":"Claude waiting","detail":"Needs input","icon":null,"timestamp_ms":null}'
```

Clear it:

```sh
zellij pipe --name tabs-clear-pane-status -- '{"type":"clear-pane-status","pane_id":{"kind":"terminal","id":1}}'
```

Or use the helper:

```bash
./tabs-status.sh status --pane terminal:1 --priority waiting --title "Claude waiting" --detail "Needs input" --icon-png /Users/robert/dev/zellij/assets/logo.png
./tabs-status.sh clear --pane terminal:1
```

The helper uses `$ZELLIJ_BIN` when set, otherwise it prefers
`/Users/robert/dev/zellij/target/dev-opt/zellij` before falling back to `zellij`.

The pipe is intentionally broadcast by name so it reaches the already-running
controller instead of launching another controller instance.
