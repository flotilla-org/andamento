# Andamento

Andamento is an experimental Zellij workflow sidebar. It started as a vertical
tab list, but now prototypes a controller/rail/config split for metadata-driven
tab grouping, templated rendering, external enrichment, and workflow-aware tab
materialization.

The crate and plugin aliases are still in transition from the older
`vertical-tabs` / `tabs-*` names. The intended published home is
`flotilla-org/andamento`, and the next rename pass should hard-cut the local
names to `andamento-*`; this prototype does not need compatibility aliases for
the old names.

## Build

```sh
cargo build --manifest-path /Users/robert/dev/zellij-scratch/Cargo.toml --workspace --target wasm32-wasip1 --release
```

This produces the current controller/rail/config artifacts:

```text
target/wasm32-wasip1/release/tabs-controller.wasm
target/wasm32-wasip1/release/tabs-rail.wasm
target/wasm32-wasip1/release/tabs-rail-config.wasm
```

## Use in Zellij

The local development layout wires the controller, rail, config plugin, and
example git watcher together:

```sh
cd /Users/robert/dev/zellij
cargo run --profile dev-opt -- --layout /Users/robert/dev/zellij-scratch/layouts/tabs-rail.kdl
```

Or use the scratch launcher, which defaults to the same dev binary:

```sh
/Users/robert/dev/zellij-scratch/run-vertical-tabs.sh
```

## Legacy Single-Plugin Prototype

The older `vertical-tabs` single-plugin experiment is still present in this
repository, but current Andamento work is happening in the controller/rail/config
prototype below. The legacy behavior was:

- active tab is highlighted
- click a row to switch tabs
- mouse wheel switches tabs
- the visible list stays centered on the active tab when there are more tabs than rows

## Controller/Rail Prototype

The current prototype is split across these plugins:

- `tabs-controller`: background state owner for pins, ordering, and pane statuses
- `tabs-rail`: visible sidebar renderer, intended to appear in each tab
- `tabs-rail-config`: in-rail settings, template diagnostics, and stats views

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

## Rail Placement

The visible rail plugin reads its physical placement from its own layout config. This is separate from controller config because placement belongs to the pane instance:

```kdl
tabs-rail location="file:/Users/robert/dev/zellij-scratch/target/wasm32-wasip1/release/tabs-rail.wasm" {
    rail_placement "left"
}
```

Supported values are `left`, `right`, `top`, and `bottom`; omitted placement defaults to `left`. The placement chooses which boundary is moved when rail widths are synchronized.

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

## Scripted Tab Factories

The example git watcher can also create managed tabs from the scripting side:

```sh
/Users/robert/dev/zellij-scratch/scripts/andamento-git-watcher.py \
  --factory-repo-manager \
  --factory-layout /Users/robert/dev/zellij-scratch/layouts/repo-manager-tab.kdl
```

When enabled, the watcher dedupes by tab name, creates one `repo: owner/name`
tab per observed git repository, captures the tab id printed by
`zellij action new-tab`, and patches that tab with typed metadata:

```text
tab.kind = repo-manager
tab.scope = GroupPath([{ key = git.repo, value = owner/name }])
```

The factory tab layout is deliberately a repeated KDL layout for now because
Zellij does not expose a slot-style way to reuse the session's tab chrome from
an external `new-tab --layout` call. The local `layouts/tabs-rail.kdl` example
embeds the controller and watcher in the first `andamento` tab and uses a
short-lived helper pane to scope that tab under an `andamento` control path.
In that embedded-controller layout, the rail alias sets
`controller_plugin_url ""`, so rail/config messages broadcast to the
already-running controller instead of launching a separate controller instance.
