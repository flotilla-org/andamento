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

## External Grouping Rules

The controller can also load grouping rules from a real filesystem path exposed to the plugin. Rules are tried by priority and project a tab's resolved metadata into a hierarchical `GroupPath`; if a rule cannot produce any segment, the next rule is tried. The local example points both config loaders at one KDL file:

```kdl
plugin location="tabs-controller" {
    grouping_config_path "/host/Users/robert/dev/zellij-scratch/templates/andamento-git.kdl"
}
```

Example grouping rules:

```kdl
grouping "proj-repo-branch" {
    priority 100
    level key="andamento.project" optional=true
    level key="git.repo" label-key="repo.name"
    level key="git.branch"
}
```

Missing optional levels are skipped. Missing later required levels stop at the deepest known level once a rule has produced a segment. If no external rule matches, `rail_grouping "directory"` still falls back to the built-in exact-cwd grouping. Use `rail_grouping "none"` or omit the setting for the flat tab rail.

## External Rail Templates

The controller can load a KDL template config from a real filesystem path exposed to the plugin, resolve templates against metadata, and send resolved fields to each rail:

```kdl
plugin location="tabs-controller" {
    template_config_path "/host/Users/robert/dev/zellij-scratch/templates/andamento-git.kdl"
}
```

Because the examples use absolute host paths under `/host`, the controller requests `FullHdAccess` and sets its plugin host folder to `/`. Template load status, errors, and resolved slots are visible in the config plugin's `templates` tab. Grouping load status is shown in the controller pane.

Example template:

```kdl
template "git.group-header" slot="group-header" node-kind="group" {
    when exists="git.repo"

    field priority=100 {
        value key="git.repo"
        value key="group.label"
    }
    field key="git.branch" priority=60 prefix=" "
}
```

Templates are matched by `slot`, `node-kind`, and `when` predicates. Field order is the render order; numeric `priority` controls which fields are dropped first when the sidebar is narrow. A `key=` value reads metadata and renders it by value type.

Switch the rail into the generic metadata inspection projection:

```kdl
plugin location="tabs-controller" {
    rail_view "metadata"
}
```

Use `rail_view "normal"` or omit the setting for the compact navigation rail.

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
