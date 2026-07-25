# Andamento

Andamento is an experimental Zellij workflow sidebar. It started as a vertical
tab list, but now prototypes a controller/rail/config split for metadata-driven
tab grouping, templated rendering, external enrichment, and workflow-aware tab
materialization.

The split prototype uses the `andamento-*` crate, plugin, artifact, and pipe
namespace throughout. The intended published home is `flotilla-org/andamento`;
this prototype does not maintain compatibility aliases for earlier scratch
names.

## Build

```sh
cargo build --manifest-path /Users/robert/dev/andamento/Cargo.toml --workspace --target wasm32-wasip1 --release
```

This produces the current controller/rail/config artifacts:

```text
target/wasm32-wasip1/release/andamento-controller.wasm
target/wasm32-wasip1/release/andamento-rail.wasm
target/wasm32-wasip1/release/andamento-config.wasm
```

## Use in Zellij

The local development layout wires the controller, rail, config plugin, and
example git watcher together:

```sh
cd /Users/robert/dev/zellij
ANDAMENTO_ROOT=/Users/robert/dev/andamento \
cargo run --profile dev-opt -- --layout /Users/robert/dev/andamento/layouts/andamento.kdl
```

Or use the scratch launcher, which defaults to the same dev binary:

```sh
/Users/robert/dev/andamento/run-andamento.sh
```

## Legacy Single-Plugin Prototype

The older `andamento` single-plugin experiment is still present in this
repository, but current Andamento work is happening in the controller/rail/config
prototype below. The legacy behavior was:

- active tab is highlighted
- click a row to switch tabs
- mouse wheel switches tabs
- the visible list stays centered on the active tab when there are more tabs than rows

## Controller/Rail Prototype

The current prototype is split across these plugins:

- `andamento-controller`: background state owner for pins, ordering, pane statuses,
  and the session-wide rail collapse/scroll snapshot
- `andamento-rail`: visible sidebar renderer, intended to appear in each tab
- `andamento-config`: in-rail settings, template diagnostics, and stats views

Rail UI synchronization deliberately uses an untargeted session broadcast. The
previous collapse path piggybacked on per-client view models addressed only to
registered renderers, so startup/registration ordering could omit an instance;
scroll position never left the originating rail at all. Rails now send actions
to the controller, consume one totally ordered collapse/scroll snapshot, and
request that current snapshot whenever a new instance starts.

## Directory Grouping

The controller can group tabs by the exact current working directory of their panes:

```kdl
plugin location="andamento-controller" {
    rail_grouping "directory"
    rail_segment_between_color "#282c34"
}
```

`rail_segment_between_color` is optional. It controls the in-between colour used by compact tab-strip separators; set it to your terminal background colour when you want those segments to blend into the rail instead of using Zellij's ribbon background.

## External Grouping Rules

The controller derives presentation paths from flat entity facts. Producers
publish `MetadataTarget::Entity({ kind, id })` patches with facts such as
`flotilla.project`, `vcs.repo`, and `flotilla.convoy`; they do not publish
group targets or paths. The bundled `flotilla.default` template orders the
spine as project → repo → convoy → vessel → session → issue → checkout.
Convoys and vessels are tab candidates, issues are section-only, and a
single vessel collapses into its convoy because both expose the same primary
action target.

The controller can also load named grouping templates from a real filesystem
path exposed to the plugin. Rules are tried by priority and project resolved
metadata into a hierarchical `GroupPath`; absent facts are skipped rather
than invented. The local example points both config loaders at one KDL file:

```kdl
plugin location="andamento-controller" {
    grouping_config_path "file:$ANDAMENTO_ROOT/templates/andamento-git.kdl"
}
```

Example grouping rules:

```kdl
grouping "proj-repo-branch" {
    priority 100
    filter key="entity.kind"
    presence kind="repo" class="tab"
    level key="andamento.project" optional=true
    level key="vcs.repo" label-key="vcs.repo.name"
    level key="git.branch"
}
```

Each level also accepts `collapse-single-member=true` and `show-empty=true`.
Presence mappings classify an entity kind as `tab`, `section`, or `hidden`.
The built-in entity renderers show the flat `source` fact as a compact
`[producer]` badge on group and tab labels.
The bundled template is available as
`templates/flotilla-default.kdl`. Switch named templates at runtime by
sending `andamento-set-grouping-template` a JSON payload such as
`{"name":"flotilla.default"}`; send `{"name":null}` to resume priority
selection. Stored entity facts are unchanged.

If no entity template matches, `rail_grouping "directory"` still falls back
to the built-in exact-cwd grouping. Use `rail_grouping "none"` or omit the
setting for the flat tab rail.

## External Rail Templates

The controller can load a KDL template config from a real filesystem path exposed to the plugin, resolve templates against metadata, and send resolved fields to each rail:

```kdl
plugin location="andamento-controller" {
    template_config_path "file:$ANDAMENTO_ROOT/templates/andamento-git.kdl"
}
```

Template load status, errors, and resolved slots are visible in the config
plugin's `templates` tab.

Example template:

```kdl
template "git.group-header" slot="group-header" node-kind="group" {
    when exists="vcs.repo"

    field priority=100 {
        value key="vcs.repo"
        value key="group.label"
    }
    field key="git.branch" priority=60 prefix=" "
}
```

Templates are matched by `slot`, `node-kind`, and `when` predicates. Field order is the render order; numeric `priority` controls which fields are dropped first when the sidebar is narrow. A `key=` value reads metadata and renders it by value type.

Switch the rail into the generic metadata inspection projection:

```kdl
plugin location="andamento-controller" {
    rail_view "metadata"
}
```

Use `rail_view "normal"` or omit the setting for the compact navigation rail.

## Rail Placement

The visible rail plugin reads its physical placement from its own layout config. This is separate from controller config because placement belongs to the pane instance:

```kdl
andamento-rail location="file:$ANDAMENTO_ROOT/target/wasm32-wasip1/release/andamento-rail.wasm" {
    rail_placement "left"
}
```

Supported values are `left`, `right`, `top`, and `bottom`; omitted placement defaults to `left`. The placement chooses which boundary is moved when rail widths are synchronized.

Set a pane status:

```sh
zellij pipe --name andamento-set-pane-status -- '{"type":"set-pane-status","pane_id":{"kind":"terminal","id":1},"priority":"waiting","title":"Claude waiting","detail":"Needs input","icon":null,"timestamp_ms":null}'
```

Clear it:

```sh
zellij pipe --name andamento-clear-pane-status -- '{"type":"clear-pane-status","pane_id":{"kind":"terminal","id":1}}'
```

Or use the helper:

```bash
./andamento-status.sh status --pane terminal:1 --priority waiting --title "Claude waiting" --detail "Needs input" --icon-png /Users/robert/dev/zellij/assets/logo.png
./andamento-status.sh clear --pane terminal:1
```

The helper uses `$ZELLIJ_BIN` when set, otherwise it prefers
`/Users/robert/dev/zellij/target/dev-opt/zellij` before falling back to `zellij`.

The pipe is intentionally broadcast by name so it reaches the already-running
controller instead of launching another controller instance.

## Scripted Tab Factories

The example git watcher can also create managed tabs from the scripting side:

```sh
/Users/robert/dev/andamento/scripts/andamento-git-watcher.py \
  --factory-repo-manager \
  --factory-layout /Users/robert/dev/andamento/layouts/repo-manager-tab.kdl
```

When enabled, the watcher dedupes by tab name, creates one `repo: owner/name`
tab per observed git repository, captures the tab id printed by
`zellij action new-tab`, and stamps that tab with canonical entity identity
and flat facts:

Like the status helper, the watcher uses `$ZELLIJ_BIN` when set and otherwise
prefers the sibling fork build at `/Users/robert/dev/zellij/target/dev-opt/zellij`
before falling back to `zellij`.

```text
entity.kind = repo
entity.id = owner/name
vcs.repo = owner/name
action.primary.target = repo-manager:owner/name
```

The factory tab layout is deliberately a repeated KDL layout for now because
Zellij does not expose a slot-style way to reuse the session's tab chrome from
an external `new-tab --layout` call. The standard `layouts/andamento.kdl` and
`layouts/andamento-native.kdl` layouts load the controller as a background
plugin, then embed a normal shell, the config plugin, and the watcher in the
first tab. The watcher is started with `--factory-repo-manager`, so observed git
repositories can materialize their repo-manager tabs automatically.
